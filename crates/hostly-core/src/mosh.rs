//! Mosh sessions. Our own SSH connection starts `mosh-server` on the host, so
//! the app's authentication and host-key trust apply as for any session; then
//! the system's `mosh-client` takes over in a local terminal.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use russh::{ChannelMsg, Disconnect, client};
use tokio::time::{Instant, timeout_at};

use crate::pty::{self, LocalCommand};
use crate::session::{SessionEvent, SessionHandle};
use crate::ssh::{ClientHandler, Connection};
use crate::{Error, Result};

const SERVER_COMMAND: &str = "mosh-server new -s -c 256 -l LANG=en_US.UTF-8";
/// How long mosh-server may take to answer, the remote shell's startup included.
const SERVER_TIMEOUT: Duration = Duration::from_secs(30);
/// After `MOSH CONNECT`, mosh-server detaches, which closes its channel. Hanging
/// up before that could kill it halfway.
const DETACH_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of mosh-server's output that an error message quotes, in characters.
const MAX_QUOTED_OUTPUT: usize = 1000;
const KEY_LENGTH: usize = 22;

impl Connection {
    /// Starts mosh-server over this connection, hangs up, and runs `client`
    /// (from [`find_mosh_client`]) in a local terminal of `cols` × `rows`,
    /// connecting to `remote_ip` (from [`resolve`]). The returned session is a
    /// local terminal's, as from [`pty::spawn`].
    pub async fn start_mosh<F>(
        self,
        remote_ip: IpAddr,
        cols: u32,
        rows: u32,
        client: &Path,
        on_event: F,
    ) -> Result<SessionHandle>
    where
        F: Fn(SessionEvent) + Send + Sync + 'static,
    {
        // Checked first, so that a missing client doesn't leave a server behind.
        if !pty::is_executable(client) {
            return Err(Error::MoshNotInstalled);
        }
        let started = start_server(&self.session).await;
        let _ = self
            .session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
        let (port, key) = started?;

        let mut env = BTreeMap::from([
            ("MOSH_KEY".to_owned(), key),
            ("TERM".to_owned(), "xterm-256color".to_owned()),
        ]);
        // mosh-client refuses to run without a UTF-8 locale, and macOS apps start with none.
        if !pty::charset_locale().is_some_and(|locale| is_utf8(&locale)) {
            for name in ["LANG", "LC_ALL"] {
                env.insert(name.to_owned(), pty::UTF8_LOCALE.to_owned());
            }
        }
        let command = LocalCommand {
            program: client.to_path_buf(),
            args: vec![remote_ip.to_string(), port.to_string()],
            env,
            cwd: None,
        };
        pty::spawn(command, cols, rows, on_event)
    }
}

/// The system's `mosh-client`. Besides `PATH`, this looks where package
/// managers install it, because apps started from the macOS Dock or Finder only
/// get a minimal `PATH`. On Windows it has to be on `PATH`.
pub fn find_mosh_client() -> Option<PathBuf> {
    #[cfg(not(windows))]
    let (name, extra_dirs): (&str, &[&str]) = (
        "mosh-client",
        &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin",
            "/usr/bin",
        ],
    );
    #[cfg(windows)]
    let (name, extra_dirs): (&str, &[&str]) = ("mosh-client.exe", &[]);
    pty::find_program(name, extra_dirs)
}

/// The address mosh-client should connect to. It's the first one the system
/// resolver returns, which is also the first one the SSH connection tries.
pub async fn resolve(host: &str, port: u16) -> Result<IpAddr> {
    let not_resolved = |reason: &dyn std::fmt::Display| {
        Error::Io(std::io::Error::other(format!(
            "couldn't resolve {host}: {reason}"
        )))
    };
    let mut addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| not_resolved(&e))?;
    addresses
        .next()
        .map(|address| address.ip())
        .ok_or_else(|| not_resolved(&"no addresses found"))
}

/// Runs mosh-server and returns the UDP port and session key it printed.
async fn start_server(session: &client::Handle<ClientHandler>) -> Result<(u16, String)> {
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, SERVER_COMMAND).await?;

    let mut output = Vec::new();
    let mut exit_status = None;
    let mut connect = None;
    let mut deadline = Instant::now() + SERVER_TIMEOUT;
    loop {
        let message = match timeout_at(deadline, channel.wait()).await {
            Ok(message) => message,
            Err(_) if connect.is_some() => break,
            Err(_) => {
                return Err(Error::Mosh(format!(
                    "no answer within {} seconds",
                    SERVER_TIMEOUT.as_secs()
                )));
            }
        };
        match message {
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                output.extend_from_slice(&data);
                if connect.is_none() {
                    connect = parse_connect(&output);
                    if connect.is_some() {
                        deadline = Instant::now() + DETACH_TIMEOUT;
                    }
                }
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => exit_status = Some(status),
            Some(ChannelMsg::Failure) => {
                return Err(Error::Mosh("the host refused to run it".into()));
            }
            Some(ChannelMsg::Close) | None => break,
            Some(_) => {}
        }
    }
    connect.ok_or_else(|| server_error(exit_status, &output))
}

/// Finds `MOSH CONNECT <port> <key>` in mosh-server's output, once the line is complete.
fn parse_connect(output: &[u8]) -> Option<(u16, String)> {
    let output = String::from_utf8_lossy(output);
    let complete = &output[..output.rfind(['\r', '\n'])?];
    complete.split(['\r', '\n']).find_map(|line| {
        let mut words = line
            .trim()
            .strip_prefix("MOSH CONNECT ")?
            .split_whitespace();
        let port = words.next()?.parse().ok().filter(|&port| port != 0)?;
        let key = words.next()?;
        let valid_key = key.len() == KEY_LENGTH
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
        (valid_key && words.next().is_none()).then(|| (port, key.to_owned()))
    })
}

/// Explains why mosh-server printed no `MOSH CONNECT` line.
fn server_error(exit_status: Option<u32>, output: &[u8]) -> Error {
    let output = String::from_utf8_lossy(output);
    let output = output.trim();
    let lowercase = output.to_lowercase();
    // 127 is the shell's "command not found"; the messages cover shells that exit otherwise.
    if exit_status == Some(127)
        || lowercase.contains("command not found")
        || lowercase.contains("mosh-server: not found")
    {
        return Error::MoshServerMissing;
    }
    Error::Mosh(match exit_status {
        _ if !output.is_empty() => quote_tail(output),
        Some(status) => format!("it exited with status {status}"),
        None => "it ended without saying why".into(),
    })
}

/// The end of `output`, where errors usually are, if it's too long to quote whole.
fn quote_tail(output: &str) -> String {
    match output.char_indices().rev().nth(MAX_QUOTED_OUTPUT - 1) {
        Some((start, _)) if start > 0 => format!("…{}", &output[start..]),
        _ => output.to_owned(),
    }
}

fn is_utf8(locale: &str) -> bool {
    let locale = locale.to_ascii_lowercase();
    locale.contains("utf-8") || locale.contains("utf8")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AbCdEfGhIjKlMnOpQrStUv";

    #[test]
    fn connect_line_is_found_among_other_output() {
        let output =
            format!("motd\r\n\r\nMOSH CONNECT 60001 {KEY}\r\n\nmosh-server (mosh 1.4.0)\n");
        assert_eq!(
            parse_connect(output.as_bytes()),
            Some((60001, KEY.to_owned()))
        );
    }

    #[test]
    fn connect_line_counts_only_once_complete() {
        let (start, end) = KEY.split_at(10);
        let mut output = format!("\r\nMOSH CONNECT 60001 {start}").into_bytes();
        assert_eq!(parse_connect(&output), None);
        output.extend_from_slice(end.as_bytes());
        assert_eq!(parse_connect(&output), None);
        output.extend_from_slice(b"\r\n");
        assert_eq!(parse_connect(&output), Some((60001, KEY.to_owned())));
    }

    #[test]
    fn malformed_connect_lines_are_ignored() {
        for line in [
            format!("MOSH CONNECT 0 {KEY}"),
            format!("MOSH CONNECT 70000 {KEY}"),
            format!("MOSH CONNECT port {KEY}"),
            format!("MOSH CONNECT 60001 {KEY}x"),
            "MOSH CONNECT 60001 AbCdEfGhIjKlMnOpQrSt-v".to_owned(),
            format!("MOSH CONNECT 60001 {KEY} extra"),
            "MOSH CONNECT 60001".to_owned(),
            format!("NOT MOSH CONNECT 60001 {KEY}"),
        ] {
            assert_eq!(
                parse_connect(format!("{line}\n").as_bytes()),
                None,
                "{line}"
            );
        }
    }

    #[test]
    fn a_missing_server_is_recognized() {
        for (status, output) in [
            (Some(127), ""),
            (Some(127), "bash: line 1: mosh-server: command not found"),
            (Some(1), "zsh:1: command not found: mosh-server"),
            (Some(2), "sh: 1: mosh-server: not found"),
        ] {
            assert!(
                matches!(
                    server_error(status, output.as_bytes()),
                    Error::MoshServerMissing
                ),
                "{output:?}"
            );
        }
    }

    #[test]
    fn other_failures_quote_the_server() {
        let error = server_error(
            Some(1),
            b"\nmosh-server needs a UTF-8 native locale to run.\n",
        );
        assert_eq!(error.kind(), "mosh");
        assert_eq!(
            error.to_string(),
            "mosh-server didn't start: mosh-server needs a UTF-8 native locale to run."
        );
        assert_eq!(
            server_error(Some(1), b"").to_string(),
            "mosh-server didn't start: it exited with status 1"
        );
    }

    #[test]
    fn long_output_is_quoted_from_its_end() {
        let output = format!("{}the actual error", "é".repeat(5000));
        let Error::Mosh(message) = server_error(Some(1), output.as_bytes()) else {
            panic!("expected a mosh error");
        };
        assert!(message.starts_with('…'));
        assert!(message.ends_with("the actual error"));
        assert_eq!(message.chars().count(), MAX_QUOTED_OUTPUT + 1);
    }

    #[test]
    fn utf8_locales_are_recognized() {
        assert!(is_utf8("en_US.UTF-8"));
        assert!(is_utf8("UTF-8"));
        assert!(is_utf8("de_DE.utf8"));
        assert!(!is_utf8("C"));
        assert!(!is_utf8("en_US.ISO8859-1"));
    }
}
