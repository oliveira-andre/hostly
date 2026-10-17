//! End-to-end port forwarding tests against an in-process russh server.

mod common;

use std::time::Duration;

use common::{PASSWORD, TestServer, start_server};
use hostly_core::Error;
use hostly_core::forward::{ForwardEvent, ForwardHandle, ForwardSpec};
use hostly_core::ssh;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(10);
const IPV4_LOOPBACK: [u8; 5] = [1, 127, 0, 0, 1];

async fn start(server: &TestServer, spec: ForwardSpec) -> Result<(ForwardHandle, Events), Error> {
    server.trust_server();
    let (sender, receiver) = mpsc::unbounded_channel();
    let handle = ssh::connect(server.request(PASSWORD))
        .await
        .unwrap()
        .start_forward(spec, move |event| {
            let _ = sender.send(event);
        })
        .await?;
    Ok((handle, Events(receiver)))
}

struct Events(mpsc::UnboundedReceiver<ForwardEvent>);

impl Events {
    /// `None` once the forward has dropped its event callback.
    async fn next(&mut self) -> Option<ForwardEvent> {
        timeout(TIMEOUT, self.0.recv())
            .await
            .expect("timed out waiting for a forward event")
    }

    async fn stopped(&mut self) -> Option<String> {
        loop {
            match self
                .next()
                .await
                .expect("the forward ended without Stopped")
            {
                ForwardEvent::Stopped { error } => return error,
                ForwardEvent::Connections { .. } | ForwardEvent::ConnectionFailed { .. } => {}
            }
        }
    }

    async fn connection_failed(&mut self) -> String {
        loop {
            match self.next().await.expect("the forward ended") {
                ForwardEvent::ConnectionFailed { error } => return error,
                ForwardEvent::Stopped { error } => panic!("the forward stopped: {error:?}"),
                ForwardEvent::Connections { .. } => {}
            }
        }
    }
}

fn local(destination_port: u16) -> ForwardSpec {
    ForwardSpec::Local {
        bind_address: "127.0.0.1".into(),
        bind_port: 0,
        destination_host: "127.0.0.1".into(),
        destination_port,
    }
}

fn remote(bind_port: u16, destination_port: u16) -> ForwardSpec {
    ForwardSpec::Remote {
        bind_address: "127.0.0.1".into(),
        bind_port,
        destination_host: "127.0.0.1".into(),
        destination_port,
    }
}

fn dynamic() -> ForwardSpec {
    ForwardSpec::Dynamic {
        bind_address: "127.0.0.1".into(),
        bind_port: 0,
    }
}

/// Echoes every byte back until the client closes; returns its port.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = socket.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    port
}

/// A port nothing listens on.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn connect_to(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).await.unwrap()
}

/// Sends `size` bytes and checks they all come back. Writes and reads at once,
/// so neither direction's buffers fill up.
async fn assert_echoes(stream: &mut TcpStream, size: usize) {
    let sent: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let (mut reader, mut writer) = stream.split();
    let write = async { writer.write_all(&sent).await.unwrap() };
    let read = async {
        let mut received = vec![0; size];
        reader.read_exact(&mut received).await.unwrap();
        received
    };
    let ((), received) = timeout(TIMEOUT, async { tokio::join!(write, read) })
        .await
        .expect("timed out waiting for the echo");
    let differs = received.iter().zip(&sent).position(|(a, b)| a != b);
    assert_eq!(differs, None, "first echoed byte that differs");
}

async fn assert_closed(stream: &mut TcpStream) {
    let mut buffer = [0; 16];
    let read = timeout(TIMEOUT, stream.read(&mut buffer))
        .await
        .expect("the connection stayed open");
    assert!(matches!(read, Ok(0) | Err(_)), "read {read:?}");
}

/// The server's listener closes a moment after it accepts the cancel.
async fn assert_eventually_refused(port: u16) {
    let refused = timeout(TIMEOUT, async {
        while TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    refused.await.expect("the port kept accepting connections");
}

/// Opens a SOCKS5 CONNECT to `address` (SOCKS5 address type and bytes) and
/// `port`; returns the stream and the proxy's reply code.
async fn socks5(proxy_port: u16, address: &[u8], port: u16) -> (TcpStream, u8) {
    let handshake = async {
        let mut stream = connect_to(proxy_port).await;
        stream.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0; 2];
        stream.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 0], "no authentication");

        let request = [&[5, 1, 0], address, &port.to_be_bytes()].concat();
        stream.write_all(&request).await.unwrap();
        let mut reply = [0; 10];
        stream.read_exact(&mut reply).await.unwrap();
        (stream, reply[1])
    };
    timeout(TIMEOUT, handshake)
        .await
        .expect("SOCKS5 handshake timed out")
}

fn domain(name: &str) -> Vec<u8> {
    [&[3, name.len() as u8], name.as_bytes()].concat()
}

#[tokio::test]
async fn local_forward_round_trips_bytes_to_the_destination() {
    let server = start_server().await;
    let (forward, mut events) = start(&server, local(echo_server().await)).await.unwrap();
    assert_ne!(forward.bound_port(), 0);

    let mut stream = connect_to(forward.bound_port()).await;
    assert_echoes(&mut stream, 1 << 20).await;
    assert_eq!(
        events.next().await,
        Some(ForwardEvent::Connections { active: 1 })
    );
    drop(stream);
    assert_eq!(
        events.next().await,
        Some(ForwardEvent::Connections { active: 0 })
    );

    forward.stop();
    assert_eq!(events.stopped().await, None);
}

#[tokio::test]
async fn remote_forward_round_trips_bytes_and_stop_cancels_it() {
    let server = start_server().await;
    let (forward, mut events) = start(&server, remote(0, echo_server().await))
        .await
        .unwrap();
    assert_ne!(forward.bound_port(), 0);

    // The test server listens on this machine too.
    let mut stream = connect_to(forward.bound_port()).await;
    assert_echoes(&mut stream, 1 << 20).await;
    drop(stream);

    forward.stop();
    assert_eq!(events.stopped().await, None);
    assert_eventually_refused(forward.bound_port()).await;
}

#[tokio::test]
async fn dynamic_forward_connects_through_socks5_by_domain_and_ipv4() {
    let server = start_server().await;
    let echo = echo_server().await;
    let (forward, _events) = start(&server, dynamic()).await.unwrap();

    for address in [domain("localhost"), IPV4_LOOPBACK.to_vec()] {
        let (mut stream, reply) = socks5(forward.bound_port(), &address, echo).await;
        assert_eq!(reply, 0, "SOCKS5 reply to {address:?}");
        assert_echoes(&mut stream, 256 * 1024).await;
    }
}

#[tokio::test]
async fn dynamic_forward_connects_through_socks4_and_socks4a() {
    let server = start_server().await;
    let echo = echo_server().await.to_be_bytes();
    let (forward, _events) = start(&server, dynamic()).await.unwrap();

    let socks4 = [&[4, 1], &echo[..], &[127, 0, 0, 1], b"user\0"].concat();
    let socks4a = [&[4, 1], &echo[..], &[0, 0, 0, 1], b"user\0localhost\0"].concat();
    for request in [socks4, socks4a] {
        let mut stream = connect_to(forward.bound_port()).await;
        stream.write_all(&request).await.unwrap();
        let mut reply = [0; 8];
        timeout(TIMEOUT, stream.read_exact(&mut reply))
            .await
            .expect("SOCKS4 reply timed out")
            .unwrap();
        assert_eq!(reply[..2], [0, 90], "SOCKS4 reply to {request:?}");
        assert_echoes(&mut stream, 64 * 1024).await;
    }
}

#[tokio::test]
async fn starting_on_a_busy_port_fails_with_a_bind_error() {
    let server = start_server().await;
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();
    let spec = ForwardSpec::Local {
        bind_address: "127.0.0.1".into(),
        bind_port: port,
        destination_host: "127.0.0.1".into(),
        destination_port: 9,
    };

    let Err(error) = start(&server, spec).await else {
        panic!("binding a busy port should fail");
    };
    assert_eq!(error.kind(), "forwardBind");
    assert!(
        error.to_string().contains(&format!("127.0.0.1:{port}")),
        "{error}"
    );
}

#[tokio::test]
async fn a_remote_forward_the_server_refuses_fails() {
    let server = start_server().await;
    // The test server can't listen on a port that's taken.
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();

    let Err(error) = start(&server, remote(port, 9)).await else {
        panic!("the server should refuse the forward");
    };
    assert_eq!(error.kind(), "forwardRefused");
    assert!(
        error.to_string().contains(&format!("127.0.0.1:{port}")),
        "{error}"
    );
}

#[tokio::test]
async fn stop_closes_the_listener_and_open_connections() {
    let server = start_server().await;
    let (forward, mut events) = start(&server, local(echo_server().await)).await.unwrap();
    let mut open = connect_to(forward.bound_port()).await;
    assert_echoes(&mut open, 1024).await;

    forward.stop();
    assert_eq!(events.stopped().await, None);
    assert_eq!(events.next().await, None, "Stopped is the last event");
    assert_closed(&mut open).await;
    assert!(
        TcpStream::connect(("127.0.0.1", forward.bound_port()))
            .await
            .is_err(),
        "new connections are refused"
    );
    forward.stop(); // Harmless once stopped.
}

#[tokio::test]
async fn a_refused_destination_closes_only_that_socks_connection() {
    let server = start_server().await;
    let echo = echo_server().await;
    let (forward, mut events) = start(&server, dynamic()).await.unwrap();
    let proxy = forward.bound_port();

    let (mut working, reply) = socks5(proxy, &IPV4_LOOPBACK, echo).await;
    assert_eq!(reply, 0);
    assert_echoes(&mut working, 1024).await;

    let dead = closed_port().await;
    let (mut refused, reply) = socks5(proxy, &IPV4_LOOPBACK, dead).await;
    assert_eq!(reply, 5, "connection refused");
    assert_closed(&mut refused).await;
    let error = events.connection_failed().await;
    assert!(error.contains(&format!("127.0.0.1:{dead}")), "{error}");

    // The open connection and the forward carry on.
    assert_echoes(&mut working, 1024).await;
    let (mut another, reply) = socks5(proxy, &IPV4_LOOPBACK, echo).await;
    assert_eq!(reply, 0);
    assert_echoes(&mut another, 1024).await;
}

#[tokio::test]
async fn local_and_remote_forwards_to_a_refused_destination_keep_listening() {
    let server = start_server().await;
    for spec in [local(closed_port().await), remote(0, closed_port().await)] {
        let (forward, mut events) = start(&server, spec).await.unwrap();
        for _ in 0..2 {
            let mut stream = connect_to(forward.bound_port()).await;
            assert_closed(&mut stream).await;
            let error = events.connection_failed().await;
            assert!(error.contains("connect"), "{error}");
        }
        forward.stop();
        assert_eq!(events.stopped().await, None);
    }
}

#[tokio::test]
async fn a_dropped_connection_stops_the_forward_with_its_reason() {
    let server = start_server().await;
    let (forward, mut events) = start(&server, local(echo_server().await)).await.unwrap();

    server.disconnect_clients("server going down").await;
    let error = events
        .stopped()
        .await
        .expect("a dropped connection is reported as an error");
    assert!(error.contains("server going down"), "{error}");
    assert!(
        TcpStream::connect(("127.0.0.1", forward.bound_port()))
            .await
            .is_err()
    );
}
