//! Remote file browsing and transfers over the SFTP subsystem.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use russh::{Disconnect, client};
use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::Metadata;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::files::{FileEntry, FileKind, sort_entries};
use crate::ssh::{ClientHandler, Connection};
use crate::{Error, Result};

const CHUNK_SIZE: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub transferred: u64,
    /// Bytes of all files in the transfer, computed before copying starts.
    pub total: u64,
    /// Source path of the file being copied.
    pub current_file: String,
}

pub struct SftpClient {
    sftp: SftpSession,
    // Owning the handle keeps the SSH connection open.
    session: client::Handle<ClientHandler>,
}

impl Connection {
    pub async fn open_sftp(self) -> Result<SftpClient> {
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;
        Ok(SftpClient {
            sftp,
            session: self.session,
        })
    }
}

/// One unit of work in a transfer, planned before any bytes move. Plans are
/// built breadth-first, so a directory always precedes its contents.
enum Step<S, T> {
    Dir { target: T },
    File { source: S, target: T, size: u64 },
}

fn total_size<S, T>(plan: &[Step<S, T>]) -> u64 {
    plan.iter()
        .map(|step| match step {
            Step::File { size, .. } => *size,
            Step::Dir { .. } => 0,
        })
        .sum()
}

impl SftpClient {
    /// The login directory, as an absolute path.
    pub async fn home(&self) -> Result<String> {
        Ok(self.sftp.canonicalize(".").await?)
    }

    pub async fn list(&self, path: &str) -> Result<Vec<FileEntry>> {
        let mut entries: Vec<FileEntry> = self
            .sftp
            .read_dir(path)
            .await?
            .map(|entry| file_entry(entry.file_name(), entry.path(), &entry.metadata()))
            .collect();
        sort_entries(&mut entries);
        Ok(entries)
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        Ok(self.sftp.create_dir(path).await?)
    }

    /// Fails if `to` exists (SFTP v3 semantics).
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(self.sftp.rename(from, to).await?)
    }

    /// Removes a file, a symlink (not its target), or a directory and everything in it.
    pub async fn remove(&self, path: &str) -> Result<()> {
        // Depth-first; a directory is removed once its children are gone.
        let mut stack = vec![(path.to_owned(), false)];
        while let Some((path, children_removed)) = stack.pop() {
            if children_removed {
                self.sftp.remove_dir(&path).await?;
            } else if self
                .sftp
                .symlink_metadata(&path)
                .await?
                .file_type()
                .is_dir()
            {
                stack.push((path.clone(), true));
                let children = self.sftp.read_dir(&path).await?;
                stack.extend(children.map(|entry| (entry.path(), false)));
            } else {
                self.sftp.remove_file(&path).await?;
            }
        }
        Ok(())
    }

    pub async fn close(&self) {
        let _ = self.sftp.close().await;
        let _ = self
            .session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }

    /// Copies a remote file or directory to `local_dir/<name>`, replacing existing files.
    /// Symlinks to files are followed; symlinked directories inside the tree are
    /// skipped to avoid cycles.
    ///
    /// Each file is written to a hidden `.<name>.hostly-part` sibling and moved
    /// into place only when complete, so a cancelled or failed transfer never
    /// touches a file that already existed.
    pub async fn download(
        &self,
        remote: &str,
        local_dir: &Path,
        on_progress: impl Fn(&TransferProgress),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let target = local_dir.join(base_name(remote)?);
        let plan = self.plan_download(remote, target).await?;
        let mut progress = Progress::new(total_size(&plan), on_progress);
        for step in plan {
            check_cancelled(cancel)?;
            match step {
                Step::Dir { target } => tokio::fs::create_dir_all(&target).await?,
                Step::File { source, target, .. } => {
                    progress.start_file(&source);
                    let part = local_part_path(&target);
                    let result: Result<()> = async {
                        self.download_file(&source, &part, &mut progress, cancel)
                            .await?;
                        if let Ok(existing) = tokio::fs::metadata(&target).await {
                            let _ = tokio::fs::set_permissions(&part, existing.permissions()).await;
                        }
                        // Atomically replaces an existing file.
                        Ok(tokio::fs::rename(&part, &target).await?)
                    }
                    .await;
                    if result.is_err() {
                        let _ = tokio::fs::remove_file(&part).await;
                    }
                    result?;
                }
            }
        }
        progress.finish();
        Ok(())
    }

    /// Copies a local file or directory to `remote_dir/<name>`, replacing existing files.
    /// Same symlink and `.hostly-part` rules as [`download`](Self::download).
    pub async fn upload(
        &self,
        local: &Path,
        remote_dir: &str,
        on_progress: impl Fn(&TransferProgress),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let name = local
            .file_name()
            .ok_or_else(|| invalid_source(&local.display().to_string()))?;
        let target = join(remote_dir, &name.to_string_lossy());
        let plan = plan_upload(local, target).await?;
        let mut progress = Progress::new(total_size(&plan), on_progress);
        for step in plan {
            check_cancelled(cancel)?;
            match step {
                Step::Dir { target } => self.ensure_dir(&target).await?,
                Step::File { source, target, .. } => {
                    progress.start_file(source.to_string_lossy());
                    let part = remote_part_path(&target);
                    let written = self
                        .upload_file(&source, &part, &mut progress, cancel)
                        .await;
                    if let Err(error) = written {
                        let _ = self.sftp.remove_file(&part).await;
                        return Err(error);
                    }
                    self.move_into_place(&part, &target).await?;
                }
            }
        }
        progress.finish();
        Ok(())
    }

    async fn plan_download(
        &self,
        remote: &str,
        target: PathBuf,
    ) -> Result<Vec<Step<String, PathBuf>>> {
        let mut plan = Vec::new();
        let mut queue = VecDeque::from([(remote.to_owned(), target)]);
        while let Some((source, target)) = queue.pop_front() {
            let metadata = self.sftp.metadata(&source).await?;
            if !metadata.file_type().is_dir() {
                plan.push(Step::File {
                    source,
                    target,
                    size: metadata.len(),
                });
                continue;
            }
            for entry in self.sftp.read_dir(&source).await? {
                let child = (entry.path(), target.join(entry.file_name()));
                let include = match entry.file_type() {
                    FileType::Dir | FileType::File => true,
                    FileType::Symlink => self
                        .sftp
                        .metadata(&child.0)
                        .await
                        .is_ok_and(|linked| !linked.file_type().is_dir()),
                    FileType::Other => false,
                };
                if include {
                    queue.push_back(child);
                }
            }
            plan.push(Step::Dir { target });
        }
        Ok(plan)
    }

    async fn download_file<F: Fn(&TransferProgress)>(
        &self,
        source: &str,
        target: &Path,
        progress: &mut Progress<F>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut remote = self.sftp.open(source).await?;
        let mut local = tokio::fs::File::create(target).await?;
        let mut buffer = vec![0; CHUNK_SIZE];
        loop {
            check_cancelled(cancel)?;
            let read = remote.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            local.write_all(&buffer[..read]).await?;
            progress.advance(read);
        }
        local.flush().await?;
        Ok(())
    }

    async fn upload_file<F: Fn(&TransferProgress)>(
        &self,
        source: &Path,
        target: &str,
        progress: &mut Progress<F>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut local = tokio::fs::File::open(source).await?;
        let mut remote = self
            .sftp
            .open_with_flags(
                target,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await?;
        let mut buffer = vec![0; CHUNK_SIZE];
        loop {
            check_cancelled(cancel)?;
            let read = local.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            remote.write_all(&buffer[..read]).await?;
            progress.advance(read);
        }
        // Waits for every write to be acknowledged, then closes the handle.
        remote.shutdown().await?;
        Ok(())
    }

    /// Renames a finished `part` over `target`. Plain SFTP v3 rename refuses to
    /// replace a file (and russh-sftp doesn't expose `posix-rename@openssh.com`),
    /// so an existing target is removed first.
    async fn move_into_place(&self, part: &str, target: &str) -> Result<()> {
        if let Ok(existing) = self.sftp.metadata(target).await
            && let Some(mode) = existing.permissions
        {
            let permissions = FileAttributes {
                permissions: Some(mode & 0o7777),
                ..FileAttributes::empty()
            };
            let _ = self.sftp.set_metadata(part, permissions).await;
        }
        let Err(rename_error) = self.sftp.rename(part, target).await else {
            return Ok(());
        };
        if self.sftp.remove_file(target).await.is_err() {
            // Nothing replaceable is in the way (e.g. a directory); the target is untouched.
            let _ = self.sftp.remove_file(part).await;
            return Err(rename_error.into());
        }
        self.sftp.rename(part, target).await.map_err(|error| {
            // The old file is gone; keep `part`, the only copy of the new one.
            Error::from(std::io::Error::other(format!(
                "{target} was replaced but the new copy couldn't be moved into place; it is saved as {part}: {error}"
            )))
        })
    }

    async fn ensure_dir(&self, path: &str) -> Result<()> {
        match self.sftp.metadata(path).await {
            Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
            // Fails with the server's reason if something else is in the way.
            _ => Ok(self.sftp.create_dir(path).await?),
        }
    }
}

async fn plan_upload(local: &Path, target: String) -> Result<Vec<Step<PathBuf, String>>> {
    let mut plan = Vec::new();
    let mut queue = VecDeque::from([(local.to_path_buf(), target)]);
    while let Some((source, target)) = queue.pop_front() {
        let metadata = tokio::fs::metadata(&source).await?;
        if !metadata.is_dir() {
            plan.push(Step::File {
                source,
                target,
                size: metadata.len(),
            });
            continue;
        }
        let mut entries = tokio::fs::read_dir(&source).await?;
        while let Some(entry) = entries.next_entry().await? {
            let child = (
                entry.path(),
                join(&target, &entry.file_name().to_string_lossy()),
            );
            let file_type = entry.file_type().await?;
            let include = if file_type.is_symlink() {
                tokio::fs::metadata(&child.0)
                    .await
                    .is_ok_and(|linked| !linked.is_dir())
            } else {
                file_type.is_dir() || file_type.is_file()
            };
            if include {
                queue.push_back(child);
            }
        }
        plan.push(Step::Dir { target });
    }
    Ok(plan)
}

/// Rate-limited progress reporting: once at the start, at most every
/// [`PROGRESS_INTERVAL`] while copying, and once at the end.
struct Progress<F> {
    state: TransferProgress,
    on_progress: F,
    last_report: Instant,
}

impl<F: Fn(&TransferProgress)> Progress<F> {
    fn new(total: u64, on_progress: F) -> Self {
        let mut progress = Self {
            state: TransferProgress {
                transferred: 0,
                total,
                current_file: String::new(),
            },
            on_progress,
            last_report: Instant::now(),
        };
        progress.report();
        progress
    }

    fn start_file(&mut self, source: impl Into<String>) {
        self.state.current_file = source.into();
    }

    fn advance(&mut self, bytes: usize) {
        self.state.transferred += bytes as u64;
        if self.last_report.elapsed() >= PROGRESS_INTERVAL {
            self.report();
        }
    }

    fn report(&mut self) {
        (self.on_progress)(&self.state);
        self.last_report = Instant::now();
    }

    fn finish(mut self) {
        self.report();
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn file_entry(name: String, path: String, metadata: &Metadata) -> FileEntry {
    FileEntry {
        name,
        path,
        kind: match metadata.file_type() {
            FileType::Dir => FileKind::Dir,
            FileType::File => FileKind::File,
            FileType::Symlink => FileKind::Symlink,
            FileType::Other => FileKind::Other,
        },
        size: metadata.len(),
        modified: metadata.mtime.map(u64::from),
        permissions: metadata.permissions.map(|mode| mode & 0o7777),
    }
}

/// SFTP paths are always `/`-separated, whatever the local platform.
fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn part_name(name: &str) -> String {
    format!(".{name}.hostly-part")
}

fn local_part_path(target: &Path) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(part_name(&name))
}

fn remote_part_path(target: &str) -> String {
    match target.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/{}", part_name(name)),
        None => part_name(target),
    }
}

fn base_name(path: &str) -> Result<&str> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or_else(|| invalid_source(path))
}

fn invalid_source(path: &str) -> Error {
    std::io::Error::new(ErrorKind::InvalidInput, format!("can't copy {path}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_splits_remote_paths() {
        assert_eq!(join("/home/me", "a.txt"), "/home/me/a.txt");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(base_name("/var/log/").unwrap(), "log");
        assert_eq!(base_name("notes.md").unwrap(), "notes.md");
        assert!(base_name("/").is_err());
        assert!(base_name("/tmp/..").is_err());
        assert_eq!(
            remote_part_path("/home/me/a.txt"),
            "/home/me/.a.txt.hostly-part"
        );
        assert_eq!(remote_part_path("/a"), "/.a.hostly-part");
        assert_eq!(
            local_part_path(Path::new("/tmp/x/b.bin")),
            PathBuf::from("/tmp/x/.b.bin.hostly-part")
        );
    }

    #[test]
    fn total_counts_file_bytes_only() {
        let plan: Vec<Step<&str, &str>> = vec![
            Step::Dir { target: "d" },
            Step::File {
                source: "a",
                target: "d/a",
                size: 3,
            },
            Step::File {
                source: "b",
                target: "d/b",
                size: 4,
            },
        ];
        assert_eq!(total_size(&plan), 7);
    }
}
