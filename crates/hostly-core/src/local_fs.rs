//! The local side of the SFTP browser.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::Result;
use crate::files::{FileEntry, FileKind, sort_entries};

/// Entries whose metadata can't be read (e.g. permission denied) are left out.
pub fn list(path: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        // `DirEntry::metadata` doesn't follow symlinks, so they are reported as links.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            FileKind::Symlink
        } else if file_type.is_dir() {
            FileKind::Dir
        } else if file_type.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            kind,
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_secs()),
            permissions: permissions(&metadata),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

#[cfg(unix)]
fn permissions(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn permissions(_: &fs::Metadata) -> Option<u32> {
    None
}

pub fn mkdir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

/// Refuses to replace an existing entry, matching SFTP rename semantics.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        )
        .into());
    }
    fs::rename(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[cfg(unix)]
    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_directories_first_then_by_name_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), "bb").unwrap();
        fs::write(dir.path().join("A.txt"), "a").unwrap();
        fs::create_dir(dir.path().join("zeta")).unwrap();
        fs::create_dir(dir.path().join("Alpha")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("zeta"), dir.path().join("link")).unwrap();

        let entries = list(dir.path()).unwrap();
        #[cfg(unix)]
        assert_eq!(names(&entries), ["Alpha", "zeta", "A.txt", "b.txt", "link"]);
        let b = entries.iter().find(|e| e.name == "b.txt").unwrap();
        assert_eq!(b.kind, FileKind::File);
        assert_eq!(b.size, 2);
        assert_eq!(b.path, dir.path().join("b.txt").to_string_lossy());
        assert!(b.modified.is_some());
        #[cfg(unix)]
        {
            let link = entries.iter().find(|e| e.name == "link").unwrap();
            assert_eq!(link.kind, FileKind::Symlink);
            assert!(b.permissions.unwrap() <= 0o7777);
        }
    }

    #[test]
    fn mkdir_fails_when_the_directory_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new");
        mkdir(&path).unwrap();
        assert!(path.is_dir());
        assert!(matches!(mkdir(&path), Err(Error::Io(_))));
    }

    #[test]
    fn rename_moves_but_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.txt");
        let to = dir.path().join("to.txt");
        fs::write(&from, "data").unwrap();
        rename(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "data");

        fs::write(&from, "other").unwrap();
        assert!(
            matches!(rename(&from, &to), Err(Error::Io(e)) if e.kind() == ErrorKind::AlreadyExists)
        );
        assert_eq!(fs::read_to_string(&to).unwrap(), "data");
    }
}
