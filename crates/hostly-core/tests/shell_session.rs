//! End-to-end shell tests against an in-process russh server.

mod common;

use std::time::Duration;

use common::{PASSWORD, TestServer, start_server};
use hostly_core::Error;
use hostly_core::ssh::{self, SessionEvent, SessionHandle};
use russh::keys::{Algorithm, HashAlg, PrivateKey, ssh_key};
use tokio::sync::mpsc;

async fn open_shell(server: &TestServer) -> (SessionHandle, Events) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let handle = ssh::connect(server.request(PASSWORD))
        .await
        .unwrap()
        .open_shell(80, 24, move |event| {
            let _ = sender.send(event);
        })
        .await
        .unwrap();
    (handle, Events(receiver))
}

async fn connect_error(server: &TestServer, password: &str) -> Error {
    match ssh::connect(server.request(password)).await {
        Ok(_) => panic!("expected the connection to fail"),
        Err(e) => e,
    }
}

struct Events(mpsc::UnboundedReceiver<SessionEvent>);

impl Events {
    async fn next(&mut self) -> SessionEvent {
        tokio::time::timeout(Duration::from_secs(5), self.0.recv())
            .await
            .expect("timed out waiting for a session event")
            .expect("event stream ended")
    }

    /// Collects output until it contains `needle`.
    async fn output_until(&mut self, needle: &str) -> String {
        let mut output = String::new();
        while !output.contains(needle) {
            match self.next().await {
                SessionEvent::Data(bytes) => output.push_str(&String::from_utf8_lossy(&bytes)),
                closed => panic!(
                    "session closed before {needle:?} arrived: {closed:?} (output: {output:?})"
                ),
            }
        }
        output
    }

    async fn closed(&mut self) -> SessionEvent {
        loop {
            if let event @ SessionEvent::Closed { .. } = self.next().await {
                return event;
            }
        }
    }
}

#[tokio::test]
async fn unknown_host_key_is_reported_and_can_then_be_trusted() {
    let server = start_server().await;

    let Error::UnknownHostKey(info) = connect_error(&server, PASSWORD).await else {
        panic!("expected an unknown host key error");
    };
    assert_eq!(info.port, server.port);
    assert_eq!(info.algorithm, "ssh-ed25519");
    assert_eq!(
        info.fingerprint,
        server.host_key.fingerprint(HashAlg::Sha256).to_string()
    );

    // The UI hands the key back exactly as it received it.
    server
        .known_hosts
        .trust_openssh(&info.host, info.port, &info.public_key)
        .unwrap();

    let (_handle, mut events) = open_shell(&server).await;
    events.output_until("welcome").await;
}

#[tokio::test]
async fn shell_round_trip_with_resize_and_exit_status() {
    let server = start_server().await;
    server.trust_server();
    let (handle, mut events) = open_shell(&server).await;

    events.output_until("welcome").await;
    handle.write(b"hello".to_vec()).unwrap();
    events.output_until("hello").await;
    handle.resize(120, 40).unwrap();
    events.output_until("resized 120x40").await;
    handle.write(b"exit\r".to_vec()).unwrap();

    assert_eq!(
        events.closed().await,
        SessionEvent::Closed {
            exit_status: Some(7),
            error: None
        }
    );
    assert!(matches!(
        handle.write(b"x".to_vec()),
        Err(Error::SessionClosed)
    ));
}

#[tokio::test]
async fn pty_is_requested_with_the_terminal_size() {
    let server = start_server().await;
    server.trust_server();
    let (_handle, mut events) = open_shell(&server).await;
    events.output_until("pty 80x24").await;
}

#[tokio::test]
async fn client_close_ends_the_session() {
    let server = start_server().await;
    server.trust_server();
    let (handle, mut events) = open_shell(&server).await;
    events.output_until("welcome").await;

    handle.close();
    assert_eq!(
        events.closed().await,
        SessionEvent::Closed {
            exit_status: None,
            error: None
        }
    );
}

#[tokio::test]
async fn wrong_password_is_an_auth_failure() {
    let server = start_server().await;
    server.trust_server();
    assert!(matches!(
        connect_error(&server, "wrong").await,
        Error::AuthFailed
    ));
}

#[tokio::test]
async fn changed_host_key_is_refused() {
    let server = start_server().await;
    let impostor = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    server
        .known_hosts
        .trust("127.0.0.1", server.port, impostor.public_key())
        .unwrap();

    let error = connect_error(&server, PASSWORD).await;
    let Error::HostKeyChanged { file, line, .. } = &error else {
        panic!("expected a changed host key error, got {error:?}");
    };
    assert_eq!(file, &server.known_hosts.app_file);
    let recorded = std::fs::read_to_string(file).unwrap();
    assert!(
        recorded
            .lines()
            .nth(line - 1)
            .unwrap()
            .contains(&impostor.public_key().to_openssh().unwrap())
    );
}

#[tokio::test]
async fn openssh_known_hosts_is_trusted_but_never_written() {
    let server = start_server().await;
    let user_file = server.known_hosts.user_file.clone().unwrap();
    let line = format!(
        "[127.0.0.1]:{} {}\n",
        server.port,
        server.host_key.to_openssh().unwrap()
    );
    std::fs::write(&user_file, &line).unwrap();

    let (_handle, mut events) = open_shell(&server).await;
    events.output_until("welcome").await;
    assert_eq!(std::fs::read_to_string(&user_file).unwrap(), line);
    assert!(!server.known_hosts.app_file.exists());
}

#[test]
fn host_key_info_round_trips_through_openssh_format() {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let info = hostly_core::HostKeyInfo::new("example.com", 22, key.public_key()).unwrap();
    let parsed: ssh_key::PublicKey = info.public_key.parse().unwrap();
    assert_eq!(&parsed, key.public_key());
}
