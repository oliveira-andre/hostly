use serde::Serialize;

/// One row of a directory listing, local or remote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    /// Symlinks are reported as such, not as what they point to.
    pub kind: FileKind,
    pub size: u64,
    /// Unix seconds.
    pub modified: Option<u64>,
    /// Permission bits only (`0o7777`), without the file type.
    pub permissions: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Directories first, then by name, ignoring case.
pub(crate) fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == FileKind::Dir;
        let b_dir = b.kind == FileKind::Dir;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sftp::TransferProgress;
    use serde_json::json;

    #[test]
    fn serializes_in_the_shape_the_ui_expects() {
        let entry = FileEntry {
            name: "a".into(),
            path: "/a".into(),
            kind: FileKind::Symlink,
            size: 1,
            modified: None,
            permissions: Some(0o644),
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({"name": "a", "path": "/a", "kind": "symlink", "size": 1, "modified": null, "permissions": 420})
        );
        let progress = TransferProgress {
            transferred: 1,
            total: 2,
            current_file: "/a".into(),
        };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({"transferred": 1, "total": 2, "currentFile": "/a"})
        );
    }
}
