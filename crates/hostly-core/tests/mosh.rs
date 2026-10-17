//! End-to-end Mosh tests: the bootstrap runs against the in-process SSH
//! server, and a shell script stands in for mosh-client.
//!
//! Unix only, because of that script.
#![cfg(unix)]

mod common;
mod events;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use common::{MOSH_KEY, MOSH_PORT, PASSWORD, TestServer, start_server};
use hostly_core::session::{SessionEvent, SessionHandle};
use hostly_core::{Error, Result, mosh, ssh};

/// A mosh-client that prints what it was started with.
fn fake_client(dir: &Path) -> PathBuf {
    let client = dir.join("mosh-client");
    std::fs::write(
        &client,
        "#!/bin/sh\nprintf 'client %s %s key=%s locale=%s\\n' \"$1\" \"$2\" \"$MOSH_KEY\" \"$LC_ALL$LC_CTYPE$LANG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
    client
}

async fn start_mosh(server: &TestServer, client: &Path) -> (Result<SessionHandle>, events::Events) {
    server.trust_server();
    let (on_event, events) = events::sink();
    let connection = ssh::connect(server.request(PASSWORD)).await.unwrap();
    let ip = mosh::resolve("127.0.0.1", server.port).await.unwrap();
    let started = connection.start_mosh(ip, 80, 24, client, on_event).await;
    (started, events)
}

#[tokio::test(flavor = "multi_thread")]
async fn mosh_server_hands_over_to_the_client() {
    let server = start_server().await;
    let dir = tempfile::tempdir().unwrap();
    let (started, events) = start_mosh(&server, &fake_client(dir.path())).await;
    let _handle = started.unwrap();

    let output = events.output_until("\r\n");
    let expected = format!("client 127.0.0.1 {MOSH_PORT} key={MOSH_KEY} ");
    assert!(output.contains(&expected), "{output:?}");
    // The client always runs with a UTF-8 locale.
    assert!(output.to_lowercase().contains("utf"), "{output:?}");
    assert_eq!(
        events.closed(),
        SessionEvent::Closed {
            exit_status: Some(0),
            error: None
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_mosh_server_is_reported() {
    let server = start_server().await;
    server.mosh_installed.store(false, Ordering::Relaxed);
    let dir = tempfile::tempdir().unwrap();
    let (started, _events) = start_mosh(&server, &fake_client(dir.path())).await;

    let error = started.unwrap_err();
    assert!(matches!(error, Error::MoshServerMissing), "{error:?}");
    assert_eq!(error.kind(), "moshServerMissing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_client_is_reported_before_the_server_starts() {
    let server = start_server().await;
    let (started, _events) = start_mosh(&server, Path::new("/nonexistent/mosh-client")).await;
    let error = started.unwrap_err();
    assert_eq!(error.kind(), "moshNotInstalled");
}

#[tokio::test]
async fn host_names_resolve_to_an_address() {
    let ip = mosh::resolve("localhost", 22).await.unwrap();
    assert!(ip.is_loopback(), "{ip}");
    assert_eq!(
        mosh::resolve("hostly.invalid", 22)
            .await
            .unwrap_err()
            .kind(),
        "io"
    );
}
