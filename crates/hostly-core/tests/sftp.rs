//! End-to-end SFTP tests against an in-process server backed by the real filesystem.
//!
//! Unix only: the test server serves real OS paths, and Windows paths
//! (`\\?\C:\...`) can't be addressed with SFTP's `/` paths. The client code
//! under test is the same on every platform.
#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::start_server;
use hostly_core::Error;
use hostly_core::files::FileKind;
use hostly_core::sftp::TransferProgress;

fn content(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed) % 251) as u8).collect()
}

/// Creates `parent/src` with nested files and an empty directory; returns the file bytes.
fn make_tree(parent: &Path) -> u64 {
    let src = parent.join("src");
    fs::create_dir_all(src.join("sub/deeper")).unwrap();
    fs::create_dir(src.join("empty")).unwrap();
    let files = [
        ("big.bin", content(700_000, 1)),
        ("notes.txt", b"hello sftp".to_vec()),
        ("sub/inner.txt", content(1_234, 2)),
        ("sub/deeper/last.bin", content(65_537, 3)),
    ];
    for (path, bytes) in &files {
        fs::write(src.join(path), bytes).unwrap();
    }
    files.iter().map(|(_, bytes)| bytes.len() as u64).sum()
}

fn assert_same_tree(expected: &Path, actual: &Path) {
    let names = |dir: &Path| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let expected_names = names(expected);
    assert_eq!(
        expected_names,
        names(actual),
        "entries of {}",
        actual.display()
    );
    for name in expected_names {
        let (expected, actual) = (expected.join(&name), actual.join(&name));
        if expected.is_dir() {
            assert_same_tree(&expected, &actual);
        } else {
            assert!(
                fs::read(&expected).unwrap() == fs::read(&actual).unwrap(),
                "{} differs",
                actual.display()
            );
        }
    }
}

/// Fails if a `.hostly-part` file is left anywhere under `dir`.
fn assert_no_part_files(dir: &Path) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        assert!(
            !path.to_string_lossy().ends_with(".hostly-part"),
            "{} was left behind",
            path.display()
        );
        if fs::symlink_metadata(&path).unwrap().is_dir() {
            assert_no_part_files(&path);
        }
    }
}

fn recorder() -> (
    Arc<Mutex<Vec<TransferProgress>>>,
    impl Fn(&TransferProgress),
) {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let sink = reports.clone();
    (reports, move |progress: &TransferProgress| {
        sink.lock().unwrap().push(progress.clone())
    })
}

#[tokio::test]
async fn lists_home_with_directories_first() {
    let server = start_server().await;
    let root = &server.sftp_root;
    fs::write(root.join("b.txt"), "bb").unwrap();
    fs::write(root.join("A.txt"), "a").unwrap();
    fs::create_dir(root.join("zeta")).unwrap();
    fs::create_dir(root.join("alpha")).unwrap();
    std::os::unix::fs::symlink(root.join("b.txt"), root.join("link")).unwrap();

    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    assert_eq!(home, root.to_string_lossy());

    let entries = sftp.list(&home).await.unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["alpha", "zeta", "A.txt", "b.txt", "link"]);
    let kind = |name: &str| entries.iter().find(|e| e.name == name).unwrap().kind;
    assert_eq!(kind("alpha"), FileKind::Dir);
    assert_eq!(kind("link"), FileKind::Symlink);

    let b = entries.iter().find(|e| e.name == "b.txt").unwrap();
    assert_eq!(b.kind, FileKind::File);
    assert_eq!(b.size, 2);
    assert_eq!(b.path, format!("{home}/b.txt"));
    assert!(b.modified.is_some());
    assert!(b.permissions.is_some_and(|mode| mode <= 0o7777));
}

#[tokio::test]
async fn mkdir_rename_and_recursive_remove() {
    let server = start_server().await;
    let root = &server.sftp_root;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();

    sftp.mkdir(&format!("{home}/new")).await.unwrap();
    assert!(root.join("new").is_dir());
    assert!(matches!(
        sftp.mkdir(&format!("{home}/new")).await,
        Err(Error::Sftp(_))
    ));

    sftp.rename(&format!("{home}/new"), &format!("{home}/renamed"))
        .await
        .unwrap();
    assert!(!root.join("new").exists());

    make_tree(&root.join("renamed"));
    sftp.remove(&format!("{home}/renamed")).await.unwrap();
    assert!(!root.join("renamed").exists());

    fs::write(root.join("single.txt"), "x").unwrap();
    sftp.remove(&format!("{home}/single.txt")).await.unwrap();
    assert!(!root.join("single.txt").exists());
}

#[tokio::test]
async fn missing_remote_paths_are_sftp_errors() {
    let server = start_server().await;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let error = sftp.list(&format!("{home}/missing")).await.unwrap_err();
    assert_eq!(error.kind(), "sftp", "got {error:?}");
}

#[tokio::test]
async fn upload_then_download_round_trips_a_directory_tree() {
    let server = start_server().await;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let total = make_tree(local.path());
    let source = local.path().join("src");

    let (reports, on_progress) = recorder();
    sftp.upload(&source, &home, on_progress, &AtomicBool::new(false))
        .await
        .unwrap();
    assert_same_tree(&source, &server.sftp_root.join("src"));
    let reports = reports.lock().unwrap().clone();
    assert_eq!(reports.first().unwrap().transferred, 0);
    assert!(reports.iter().all(|report| report.total == total));
    assert_eq!(reports.last().unwrap().transferred, total);

    let back = tempfile::tempdir().unwrap();
    let (reports, on_progress) = recorder();
    sftp.download(
        &format!("{home}/src"),
        back.path(),
        on_progress,
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    assert_same_tree(&source, &back.path().join("src"));
    assert_no_part_files(&server.sftp_root);
    assert_no_part_files(back.path());
    let last = reports.lock().unwrap().last().cloned().unwrap();
    assert_eq!((last.transferred, last.total), (total, total));
    assert!(last.current_file.starts_with(&format!("{home}/src/")));
}

#[tokio::test]
async fn transfers_replace_existing_files_and_keep_their_permissions() {
    let server = start_server().await;
    let root = &server.sftp_root;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let file = local.path().join("deploy.sh");
    let idle = AtomicBool::new(false);

    fs::write(root.join("deploy.sh"), "an older, longer remote version").unwrap();
    set_mode(&root.join("deploy.sh"), 0o750);
    fs::write(&file, "new").unwrap();
    sftp.upload(&file, &home, |_| {}, &idle).await.unwrap();
    assert_eq!(fs::read_to_string(root.join("deploy.sh")).unwrap(), "new");
    assert_mode(&root.join("deploy.sh"), 0o750);

    fs::write(root.join("deploy.sh"), "remote").unwrap();
    set_mode(&file, 0o700);
    sftp.download(&format!("{home}/deploy.sh"), local.path(), |_| {}, &idle)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "remote");
    assert_mode(&file, 0o700);

    assert_no_part_files(root);
    assert_no_part_files(local.path());
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn assert_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o7777,
        mode
    );
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

#[cfg(not(unix))]
fn assert_mode(_: &Path, _: u32) {}

#[tokio::test]
async fn a_failed_replace_keeps_the_original_and_cleans_up() {
    let server = start_server().await;
    let root = &server.sftp_root;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let idle = AtomicBool::new(false);

    // A non-empty directory where the file should go can't be replaced.
    fs::create_dir_all(root.join("report.txt/keep")).unwrap();
    fs::write(local.path().join("report.txt"), "new").unwrap();
    let upload = sftp
        .upload(&local.path().join("report.txt"), &home, |_| {}, &idle)
        .await;
    assert!(upload.is_err());
    assert!(root.join("report.txt/keep").is_dir());

    fs::write(root.join("notes.txt"), "remote").unwrap();
    fs::create_dir_all(local.path().join("notes.txt/keep")).unwrap();
    let download = sftp
        .download(&format!("{home}/notes.txt"), local.path(), |_| {}, &idle)
        .await;
    assert!(download.is_err());
    assert!(local.path().join("notes.txt/keep").is_dir());

    assert_no_part_files(root);
    assert_no_part_files(local.path());
}

#[tokio::test]
async fn file_links_are_followed_and_directory_links_skipped() {
    let server = start_server().await;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("real.txt"), "real").unwrap();
    std::os::unix::fs::symlink(src.join("real.txt"), src.join("alias.txt")).unwrap();
    // A link back to its own parent would recurse forever if followed.
    std::os::unix::fs::symlink(&src, src.join("loop")).unwrap();

    sftp.upload(&src, &home, |_| {}, &AtomicBool::new(false))
        .await
        .unwrap();
    let uploaded = server.sftp_root.join("src");
    assert_eq!(
        fs::read_to_string(uploaded.join("alias.txt")).unwrap(),
        "real"
    );
    assert!(
        !fs::symlink_metadata(uploaded.join("alias.txt"))
            .unwrap()
            .is_symlink()
    );
    assert!(!uploaded.join("loop").exists());
}

#[tokio::test]
async fn a_cancelled_transfer_creates_nothing() {
    let server = start_server().await;
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    make_tree(local.path());
    let cancelled = AtomicBool::new(true);

    let result = sftp
        .upload(&local.path().join("src"), &home, |_| {}, &cancelled)
        .await;
    assert!(matches!(result, Err(Error::Cancelled)), "got {result:?}");
    assert!(!server.sftp_root.join("src").exists());
}

/// Cancels once `part` has some bytes in it, then checks it was removed and
/// `original` (an existing file the transfer would replace) is untouched.
async fn cancel_midway(
    part: &Path,
    original: &Path,
    transfer: impl AsyncFnOnce(&AtomicBool) -> hostly_core::Result<()>,
) {
    let before = fs::read(original).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (cancel, part) = (cancel.clone(), part.to_path_buf());
        tokio::spawn(async move {
            while !fs::metadata(&part).is_ok_and(|m| m.len() > 0) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            cancel.store(true, Ordering::Relaxed);
        })
    };
    let result = transfer(&cancel).await;
    watcher.abort();
    assert!(matches!(result, Err(Error::Cancelled)), "got {result:?}");
    assert!(!part.exists(), "{} was left behind", part.display());
    assert!(
        fs::read(original).unwrap() == before,
        "{} changed",
        original.display()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_download_keeps_the_existing_local_file() {
    let server = start_server().await;
    fs::write(server.sftp_root.join("huge.bin"), content(48 << 20, 4)).unwrap();
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let original = local.path().join("huge.bin");
    fs::write(&original, content(10_000, 9)).unwrap();

    let part = local.path().join(".huge.bin.hostly-part");
    cancel_midway(&part, &original, async |cancel| {
        sftp.download(&format!("{home}/huge.bin"), local.path(), |_| {}, cancel)
            .await
    })
    .await;
    assert_no_part_files(local.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_upload_keeps_the_existing_remote_file() {
    let server = start_server().await;
    let original = server.sftp_root.join("huge.bin");
    fs::write(&original, content(10_000, 8)).unwrap();
    let sftp = server.sftp().await;
    let home = sftp.home().await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let file = local.path().join("huge.bin");
    fs::write(&file, content(48 << 20, 5)).unwrap();

    let part = server.sftp_root.join(".huge.bin.hostly-part");
    cancel_midway(&part, &original, async |cancel| {
        sftp.upload(&file, &home, |_| {}, cancel).await
    })
    .await;
    assert_no_part_files(&server.sftp_root);
}
