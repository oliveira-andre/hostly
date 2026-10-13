//! Small JSON files in the app data directory.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Result;

/// Reads `path`, or `None` if it doesn't exist yet.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write-then-rename, so a crash mid-write can't leave a truncated file.
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
