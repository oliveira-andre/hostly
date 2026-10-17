//! Local terminals: a program, usually the user's shell, in a pseudo-terminal
//! (ConPTY on Windows), with the same event contract as SSH shells.

use std::collections::BTreeMap;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{
    ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtyPair, PtySize, native_pty_system,
};
use tokio::sync::mpsc;

use crate::session::{Command, Emitter, SessionEvent, SessionHandle};
use crate::{Error, Result};

/// How long a program's last output may take to arrive after it exits. It only
/// runs out when a background job the program started still holds the terminal.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// The locale given to programs that would otherwise run without a UTF-8 one.
pub(crate) const UTF8_LOCALE: &str = "en_US.UTF-8";

/// A program to run in a local terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalCommand {
    /// A path, or a name to look up on `PATH`.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the app's own environment, replacing variables of the same name.
    pub env: BTreeMap<String, String>,
    /// `None`, or a directory that doesn't exist, starts in the home directory.
    pub cwd: Option<PathBuf>,
}

/// The user's shell, started in their home directory with `TERM=xterm-256color`.
///
/// On Unix that's `$SHELL` (else `/bin/sh`) as a login shell, so it reads the
/// profile files that set up PATH: apps started from the macOS Dock or Finder
/// only get a minimal one. When no locale is set, which is also the case for
/// those apps, it gets a UTF-8 `LANG`.
///
/// On Windows it's PowerShell, as in Windows Terminal: PowerShell 7 (`pwsh.exe`)
/// when it's on PATH, else the Windows PowerShell that every Windows includes.
/// cmd.exe (`COMSPEC`) would be the legacy choice.
pub fn default_shell() -> LocalCommand {
    let mut env = BTreeMap::from([
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
    ]);
    // Windows programs don't read LANG.
    if cfg!(not(windows)) && charset_locale().is_none() {
        env.insert("LANG".to_owned(), UTF8_LOCALE.to_owned());
    }

    #[cfg(not(windows))]
    let (program, args) = {
        let shell = std::env::var_os("SHELL").filter(|shell| !shell.is_empty());
        let program = shell.map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from);
        (program, vec!["-l".to_owned()])
    };
    #[cfg(windows)]
    let (program, args) = {
        let program = find_program("pwsh.exe", &[]).unwrap_or_else(|| {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
        });
        (program, vec!["-NoLogo".to_owned()])
    };

    LocalCommand {
        program,
        args,
        env,
        cwd: std::env::home_dir(),
    }
}

/// Starts `command` in a new terminal of `cols` × `rows`.
///
/// Output and then exactly one `Closed` are delivered through `on_event`.
/// `Closed` carries the program's exit status, or `None` when a signal ended
/// it or the session was closed from this side. [`SessionHandle::close`] ends
/// the program (on Unix with SIGHUP, as closing a terminal window does).
///
/// The blocking terminal I/O runs on threads of its own, so this can be called
/// from anywhere, inside an async runtime or not.
pub fn spawn<F>(command: LocalCommand, cols: u32, rows: u32, on_event: F) -> Result<SessionHandle>
where
    F: Fn(SessionEvent) + Send + Sync + 'static,
{
    let PtyPair { master, slave } = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(pty_error)?;
    let reader = master.try_clone_reader().map_err(pty_error)?;
    let mut child = slave.spawn_command(builder(command)).map_err(pty_error)?;
    // Only the program may keep this end open, or its exit would go unnoticed.
    drop(slave);
    // Taken only once the program runs: dropping the writer types an end of
    // input, and on macOS that makes tearing down a failed terminal hang.
    let writer = match master.take_writer() {
        Ok(writer) => writer,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(pty_error(e));
        }
    };
    let killer = child.clone_killer();

    let (commands, receiver) = mpsc::unbounded_channel();
    // Weak, so that dropping every handle still ends the session.
    let wake = commands.downgrade();
    let session = Arc::new(Session {
        events: Emitter::new(on_event),
        exit: Mutex::new(None),
    });
    let (output_done, output_ended) = channel::<()>();
    let (input, input_queue) = channel();

    let output_session = session.clone();
    thread::spawn(move || {
        read_output(reader, &output_session.events);
        drop(output_done);
    });
    // Input has a thread of its own so that a program that stops reading it
    // can't hold up a close.
    thread::spawn(move || write_input(writer, input_queue));
    let exit_session = session.clone();
    thread::spawn(move || {
        let exit = child.wait();
        *exit_session.exit.lock().unwrap() = Some(exit);
        if let Some(commands) = wake.upgrade() {
            let _ = commands.send(Command::Close);
        }
    });
    thread::spawn(move || {
        let control = Control {
            master,
            input,
            killer,
            output_ended,
        };
        control.run(receiver, &session);
    });

    Ok(SessionHandle { commands })
}

struct Session<F> {
    events: Emitter<F>,
    /// The program's exit, recorded by the thread that waits for it.
    exit: Mutex<Option<io::Result<ExitStatus>>>,
}

/// What the thread that serves the [`SessionHandle`] needs.
struct Control {
    master: Box<dyn MasterPty + Send>,
    input: Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    output_ended: Receiver<()>,
}

impl Control {
    /// Serves commands until a close, from either side, then sends `Closed`.
    fn run<F: Fn(SessionEvent)>(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        session: &Session<F>,
    ) {
        while let Some(command) = commands.blocking_recv() {
            match command {
                Command::Write(data) => {
                    let _ = self.input.send(data);
                }
                // A failed resize keeps the old size, which isn't worth ending the session over.
                Command::Resize { cols, rows } => {
                    let _ = self.master.resize(size(cols, rows));
                }
                Command::Close => break,
            }
        }
        commands.close();

        let exit = session.exit.lock().unwrap().take();
        let Some(exit) = exit else {
            // Closed from this side while the program still runs.
            let _ = self.killer.kill();
            session.events.close(None, None);
            return;
        };
        // On Windows the program's last output only ends once ConPTY is closed.
        drop(self.input);
        drop(self.master);
        let _ = self.output_ended.recv_timeout(DRAIN_TIMEOUT);
        match exit {
            Ok(status) => {
                let exit_status = status.signal().is_none().then_some(status.exit_code());
                session.events.close(exit_status, None);
            }
            Err(e) => session.events.close(None, Some(e.to_string())),
        }
    }
}

/// Forwards the program's output until its end of the terminal closes.
fn read_output<F: Fn(SessionEvent)>(mut reader: Box<dyn Read + Send>, events: &Emitter<F>) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if !events.data(&buffer[..read]) {
            break;
        }
    }
}

fn write_input(mut writer: Box<dyn Write + Send>, queue: Receiver<Vec<u8>>) {
    for data in queue {
        if writer
            .write_all(&data)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

fn builder(command: LocalCommand) -> CommandBuilder {
    let mut builder = CommandBuilder::new(command.program);
    builder.args(command.args);
    for (name, value) in command.env {
        builder.env(name, value);
    }
    if let Some(cwd) = command.cwd {
        builder.cwd(cwd);
    }
    builder
}

fn size(cols: u32, rows: u32) -> PtySize {
    let clamp = |n: u32| n.clamp(1, u16::MAX.into()) as u16;
    PtySize {
        rows: clamp(rows),
        cols: clamp(cols),
        ..PtySize::default()
    }
}

fn pty_error(error: impl std::fmt::Display) -> Error {
    // `#` prints an `anyhow::Error` with its causes.
    Error::Pty(format!("{error:#}"))
}

/// The locale that decides the character set: the first of `LC_ALL`,
/// `LC_CTYPE` and `LANG` that is set.
pub(crate) fn charset_locale() -> Option<String> {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// Looks for an executable on `PATH`, then in `extra_dirs`.
pub(crate) fn find_program(name: &str, extra_dirs: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(extra_dirs.iter().map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_starts_at_home_with_xterm_colors() {
        let shell = default_shell();
        assert_eq!(shell.env["TERM"], "xterm-256color");
        assert_eq!(shell.cwd, std::env::home_dir());
        #[cfg(unix)]
        assert_eq!(shell.args, ["-l"]);
    }

    #[test]
    fn sizes_are_clamped_to_what_a_terminal_holds() {
        let size = size(0, 100_000);
        assert_eq!((size.cols, size.rows), (1, u16::MAX));
    }

    #[cfg(unix)]
    #[test]
    fn programs_are_found_only_when_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("hostly-test-program");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        let extra = [dir.path().to_str().unwrap()];
        assert_eq!(find_program("hostly-test-program", &extra), None);

        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program("hostly-test-program", &extra), Some(program));
    }
}
