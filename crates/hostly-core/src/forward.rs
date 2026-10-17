//! SSH port forwarding: local (`ssh -L`), remote (`ssh -R`) and dynamic SOCKS (`ssh -D`).
//!
//! A forward owns its SSH connection. Each tunneled TCP connection runs in its
//! own task, so one that fails closes alone and the forward keeps serving.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use russh::{Channel, ChannelOpenFailure, Disconnect, client};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::ssh::{ClientHandler, Connection};
use crate::{Error, Result};

type Ssh = client::Handle<ClientHandler>;

/// How long a SOCKS client may take to send its request.
const SOCKS_TIMEOUT: Duration = Duration::from_secs(30);
/// How long stopping waits for the server to cancel a remote forward.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
/// Pause after a failed `accept`, which is usually transient (out of file descriptors).
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// What to forward. Bind addresses follow OpenSSH: empty means loopback only,
/// `*` means every interface; IPv6 addresses go without brackets. A
/// `bind_port` of 0 picks a free port, reported by [`ForwardHandle::bound_port`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ForwardSpec {
    /// Listens on this machine. The server connects each connection to the
    /// destination, resolving its name itself.
    Local {
        bind_address: String,
        bind_port: u16,
        destination_host: String,
        destination_port: u16,
    },
    /// Asks the server to listen. This machine connects each connection to the
    /// destination. Listening beyond loopback needs the server's `GatewayPorts`.
    Remote {
        bind_address: String,
        bind_port: u16,
        destination_host: String,
        destination_port: u16,
    },
    /// A SOCKS4, SOCKS4a and SOCKS5 proxy on this machine. The server connects
    /// to whatever each client asks for, resolving names itself.
    Dynamic {
        bind_address: String,
        bind_port: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ForwardEvent {
    /// The number of open tunneled connections changed.
    Connections { active: usize },
    /// One connection couldn't be tunneled and was closed; the forward keeps running.
    ConnectionFailed { error: String },
    /// Always the last event, sent exactly once. `error` is `None` after
    /// [`ForwardHandle::stop`] and says why otherwise. No connections remain.
    Stopped { error: Option<String> },
}

/// Controls a running forward. Cheap to clone; dropping every clone stops it too.
#[derive(Clone, Debug)]
pub struct ForwardHandle {
    stop: mpsc::UnboundedSender<()>,
    bound_port: u16,
}

impl ForwardHandle {
    /// Closes the listener and every tunneled connection, cancels a remote
    /// forward and disconnects. Returns at once; [`ForwardEvent::Stopped`] follows.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// The port actually listening, which differs from the spec's when it asked for 0.
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }
}

/// What the server sends a [`Connection`] outside the client's own channels.
pub(crate) enum Incoming {
    /// A connection to a remote forward's port, not yet accepted.
    Channel {
        channel: Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
    },
    /// The connection ended; the server's or the transport's reason, possibly empty.
    Disconnected(String),
}

impl Connection {
    /// Starts forwarding. Resolves once it's listening: bound on this machine,
    /// or accepted by the server. The forward owns the connection and
    /// disconnects it when it ends. `on_event` may run before this returns.
    pub async fn start_forward<F>(self, spec: ForwardSpec, on_event: F) -> Result<ForwardHandle>
    where
        F: Fn(ForwardEvent) + Send + Sync + 'static,
    {
        let ssh = Arc::new(self.session);
        let started = match spec {
            ForwardSpec::Local {
                bind_address,
                bind_port,
                destination_host,
                destination_port,
            } => {
                Forward::listen(
                    &bind_address,
                    bind_port,
                    Some((destination_host, destination_port)),
                )
                .await
            }
            ForwardSpec::Dynamic {
                bind_address,
                bind_port,
            } => Forward::listen(&bind_address, bind_port, None).await,
            ForwardSpec::Remote {
                bind_address,
                bind_port,
                destination_host,
                destination_port,
            } => {
                Forward::request(
                    &ssh,
                    &bind_address,
                    bind_port,
                    destination_host,
                    destination_port,
                )
                .await
            }
        };
        let (forward, bound_port) = match started {
            Ok(started) => started,
            Err(e) => {
                let _ = ssh.disconnect(Disconnect::ByApplication, "", "en").await;
                return Err(e);
            }
        };
        let (stop, stop_receiver) = mpsc::unbounded_channel();
        tokio::spawn(forward.run(ssh, self.incoming, stop_receiver, on_event));
        Ok(ForwardHandle { stop, bound_port })
    }
}

/// A forward that is listening, before its loop starts.
struct Forward {
    /// Where local and dynamic forwards accept connections.
    listener: Option<TcpListener>,
    /// The fixed destination; `None` for SOCKS, whose clients name their own.
    destination: Option<(String, u16)>,
    /// A remote forward's address and port as the server accepted them, to cancel it.
    remote: Option<(String, u16)>,
}

impl Forward {
    async fn listen(
        bind_address: &str,
        port: u16,
        destination: Option<(String, u16)>,
    ) -> Result<(Self, u16)> {
        // `localhost` may resolve to ::1 alone, which IPv4 clients can't reach.
        let host = match bind_address {
            "" | "localhost" => "127.0.0.1",
            "*" => "0.0.0.0",
            host => host,
        };
        let bind_error = |source| Error::ForwardBind {
            address: host_port(host, port),
            source,
        };
        let listener = TcpListener::bind((host, port)).await.map_err(bind_error)?;
        let bound_port = listener.local_addr().map_err(bind_error)?.port();
        let forward = Self {
            listener: Some(listener),
            destination,
            remote: None,
        };
        Ok((forward, bound_port))
    }

    async fn request(
        ssh: &Ssh,
        bind_address: &str,
        port: u16,
        destination_host: String,
        destination_port: u16,
    ) -> Result<(Self, u16)> {
        // The SSH protocol's spelling: "" is every interface.
        let address = match bind_address {
            "" => "localhost",
            "*" => "",
            address => address,
        };
        let allocated = match ssh.tcpip_forward(address, port.into()).await {
            Ok(allocated) => allocated,
            Err(russh::Error::RequestDenied) => {
                let shown = if address.is_empty() { "*" } else { address };
                return Err(Error::ForwardRefused {
                    address: host_port(shown, port),
                });
            }
            Err(e) => return Err(e.into()),
        };
        // The server reports a port only when it picked one.
        let bound_port = u16::try_from(allocated)
            .ok()
            .filter(|&allocated| allocated != 0)
            .unwrap_or(port);
        let forward = Self {
            listener: None,
            destination: Some((destination_host, destination_port)),
            remote: Some((address.to_owned(), bound_port)),
        };
        Ok((forward, bound_port))
    }

    async fn run<F>(
        self,
        ssh: Arc<Ssh>,
        mut incoming: mpsc::UnboundedReceiver<Incoming>,
        mut stop: mpsc::UnboundedReceiver<()>,
        on_event: F,
    ) where
        F: Fn(ForwardEvent) + Send + Sync + 'static,
    {
        let Forward {
            listener,
            destination,
            remote,
        } = self;
        // One task per tunneled connection; shutting the set down aborts them all.
        let mut tunnels = JoinSet::new();
        let error = loop {
            let active = tunnels.len();
            tokio::select! {
                _ = stop.recv() => break None,
                event = incoming.recv() => match event {
                    Some(Incoming::Channel { channel, reply }) => match (&remote, &destination) {
                        (Some(_), Some((host, port))) => {
                            tunnels.spawn(connect_forwarded(channel, reply, host.clone(), *port));
                        }
                        // Dropping `reply` refuses a channel this forward didn't ask for.
                        _ => drop(reply),
                    },
                    Some(Incoming::Disconnected(reason)) => break Some(connection_lost(&reason)),
                    None => break Some(connection_lost("")),
                },
                accepted = accept(listener.as_ref()) => match accepted {
                    Ok((socket, peer)) => {
                        tunnels.spawn(tunnel(ssh.clone(), socket, peer, destination.clone()));
                    }
                    Err(_) => tokio::time::sleep(ACCEPT_RETRY).await,
                },
                Some(joined) = tunnels.join_next(), if !tunnels.is_empty() => {
                    if let Ok(Err(error)) = joined {
                        on_event(ForwardEvent::ConnectionFailed { error });
                    }
                }
            }
            if tunnels.len() != active {
                on_event(ForwardEvent::Connections {
                    active: tunnels.len(),
                });
            }
        };

        drop(listener);
        tunnels.shutdown().await;
        if let (None, Some((address, port))) = (&error, remote) {
            let cancel = ssh.cancel_tcpip_forward(address, port.into());
            let _ = tokio::time::timeout(CANCEL_TIMEOUT, cancel).await;
        }
        let _ = ssh.disconnect(Disconnect::ByApplication, "", "en").await;
        on_event(ForwardEvent::Stopped { error });
    }
}

/// Accepts from `listener`, or never when there is none (remote forwards).
async fn accept(listener: Option<&TcpListener>) -> io::Result<(TcpStream, SocketAddr)> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

fn connection_lost(reason: &str) -> String {
    if reason.is_empty() {
        "the SSH connection was closed".into()
    } else {
        format!("the SSH connection was closed: {reason}")
    }
}

/// `host:port`, with IPv6 addresses in brackets.
fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Tunnels one accepted connection through a `direct-tcpip` channel, to the
/// fixed `destination` or, without one, where the SOCKS client asks.
async fn tunnel(
    ssh: Arc<Ssh>,
    mut socket: TcpStream,
    peer: SocketAddr,
    destination: Option<(String, u16)>,
) -> Result<(), String> {
    let _ = socket.set_nodelay(true);
    let (host, port, socks_version) = match destination {
        Some((host, port)) => (host, port, None),
        None => {
            let request = tokio::time::timeout(SOCKS_TIMEOUT, read_socks_request(&mut socket))
                .await
                .map_err(|_| "the SOCKS client sent no request".to_owned())??;
            (request.host, request.port, Some(request.version))
        }
    };
    let opened = ssh
        .channel_open_direct_tcpip(
            host.as_str(),
            port.into(),
            peer.ip().to_string(),
            peer.port().into(),
        )
        .await;
    let channel = match opened {
        Ok(channel) => channel,
        Err(e) => {
            if let Some(version) = socks_version {
                let _ = socks_reply(&mut socket, version, SocksReply::for_error(&e)).await;
            }
            return Err(open_error(&host, port, &e));
        }
    };
    if let Some(version) = socks_version {
        socks_reply(&mut socket, version, SocksReply::Granted)
            .await
            .map_err(|e| format!("SOCKS: {e}"))?;
    }
    pump(socket, channel).await;
    Ok(())
}

/// Connects a remote forward's channel to its destination from this machine.
async fn connect_forwarded(
    channel: Channel<client::Msg>,
    reply: client::ChannelOpenHandle,
    host: String,
    port: u16,
) -> Result<(), String> {
    match TcpStream::connect((host.as_str(), port)).await {
        Ok(socket) => {
            let _ = socket.set_nodelay(true);
            reply.accept().await;
            pump(socket, channel).await;
            Ok(())
        }
        Err(e) => {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            Err(format!(
                "couldn't connect to {}: {e}",
                host_port(&host, port)
            ))
        }
    }
}

/// Copies both ways until both sides finish or either fails, then closes both.
async fn pump(mut socket: TcpStream, channel: Channel<client::Msg>) {
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
}

fn open_error(host: &str, port: u16, error: &russh::Error) -> String {
    let reason = match error {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => {
            "the server couldn't connect to it".to_owned()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            "the server doesn't allow forwarding to it".to_owned()
        }
        e => e.to_string(),
    };
    format!("couldn't tunnel to {}: {reason}", host_port(host, port))
}

/// A SOCKS client's CONNECT request.
#[derive(Debug, PartialEq, Eq)]
struct SocksRequest {
    /// 4 (also for SOCKS4a) or 5; replies use the same version.
    version: u8,
    host: String,
    port: u16,
}

#[derive(Clone, Copy, Debug)]
enum SocksReply {
    Granted,
    Failed,
    NotAllowed,
    Refused,
    CommandNotSupported,
    AddressNotSupported,
}

impl SocksReply {
    fn for_error(error: &russh::Error) -> Self {
        match error {
            russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => Self::Refused,
            russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
                Self::NotAllowed
            }
            _ => Self::Failed,
        }
    }
}

async fn socks_reply<S>(stream: &mut S, version: u8, reply: SocksReply) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    if version == 4 {
        let code = if matches!(reply, SocksReply::Granted) {
            90
        } else {
            91
        };
        // The address fields are ignored for CONNECT.
        return stream.write_all(&[0, code, 0, 0, 0, 0, 0, 0]).await;
    }
    let code = match reply {
        SocksReply::Granted => 0,
        SocksReply::Failed => 1,
        SocksReply::NotAllowed => 2,
        SocksReply::Refused => 5,
        SocksReply::CommandNotSupported => 7,
        SocksReply::AddressNotSupported => 8,
    };
    // The bound address is unknown on this side of the tunnel, so it is 0.0.0.0:0.
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

const SOCKS_CONNECT: u8 = 1;
const SOCKS5_NO_AUTH: u8 = 0;
const SOCKS5_NO_ACCEPTABLE_METHOD: u8 = 0xFF;

/// Reads a SOCKS4, SOCKS4a or SOCKS5 CONNECT request. A request this proxy
/// can't serve is answered with a failure before the error is returned.
async fn read_socks_request<S>(stream: &mut S) -> Result<SocksRequest, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = match stream.read_u8().await {
        Ok(4) => read_socks4(stream).await,
        Ok(5) => read_socks5(stream).await,
        Ok(version) => Err(unsupported(format!("version {version}"))),
        Err(e) => Err(e),
    };
    request.map_err(|e| format!("SOCKS: {e}"))
}

fn unsupported(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("unsupported {what}"))
}

/// The rest of a SOCKS4 or SOCKS4a request, after its version byte.
async fn read_socks4<S>(stream: &mut S) -> io::Result<SocksRequest>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let command = stream.read_u8().await?;
    let port = stream.read_u16().await?;
    let mut ip = [0; 4];
    stream.read_exact(&mut ip).await?;
    let _user_id = read_nul_terminated(stream).await?;
    // SOCKS4a: an address of 0.0.0.x, x nonzero, means a host name follows.
    let host = match ip {
        [0, 0, 0, last] if last != 0 => read_nul_terminated(stream).await?,
        ip => Ipv4Addr::from(ip).to_string(),
    };
    if command != SOCKS_CONNECT {
        socks_reply(stream, 4, SocksReply::CommandNotSupported).await?;
        return Err(unsupported(format!("command {command}")));
    }
    Ok(SocksRequest {
        version: 4,
        host,
        port,
    })
}

/// The rest of a SOCKS5 greeting and request, after the greeting's version byte.
async fn read_socks5<S>(stream: &mut S) -> io::Result<SocksRequest>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let method_count = stream.read_u8().await?;
    let mut methods = vec![0; method_count.into()];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&SOCKS5_NO_AUTH) {
        stream.write_all(&[5, SOCKS5_NO_ACCEPTABLE_METHOD]).await?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the client requires authentication",
        ));
    }
    stream.write_all(&[5, SOCKS5_NO_AUTH]).await?;

    let [version, command, _reserved, address_type] = {
        let mut header = [0; 4];
        stream.read_exact(&mut header).await?;
        header
    };
    if version != 5 {
        return Err(unsupported(format!("version {version}")));
    }
    let host = match address_type {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip).await?;
            Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut name = vec![0; stream.read_u8().await?.into()];
            stream.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip).await?;
            Ipv6Addr::from(ip).to_string()
        }
        _ => {
            // The address's length is unknown, so the request can't be read further.
            socks_reply(stream, 5, SocksReply::AddressNotSupported).await?;
            return Err(unsupported(format!("address type {address_type}")));
        }
    };
    let port = stream.read_u16().await?;
    if command != SOCKS_CONNECT {
        socks_reply(stream, 5, SocksReply::CommandNotSupported).await?;
        return Err(unsupported(format!("command {command}")));
    }
    Ok(SocksRequest {
        version: 5,
        host,
        port,
    })
}

/// Reads a SOCKS4 string, up to 255 bytes plus its terminating NUL.
async fn read_nul_terminated<S>(stream: &mut S) -> io::Result<String>
where
    S: AsyncRead + Unpin,
{
    // Byte by byte: anything after the request belongs to the tunnel.
    let mut bytes = Vec::new();
    loop {
        match stream.read_u8().await? {
            0 => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            _ if bytes.len() == 255 => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "name too long"));
            }
            byte => bytes.push(byte),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::*;

    /// Feeds `request` to the parser; returns what it parsed and what it answered.
    async fn parse(request: &[u8]) -> (Result<SocksRequest, String>, Vec<u8>) {
        let (mut client, mut proxy) = duplex(1024);
        client.write_all(request).await.unwrap();
        let parsed = read_socks_request(&mut proxy).await;
        drop(proxy);
        let mut answered = Vec::new();
        client.read_to_end(&mut answered).await.unwrap();
        (parsed, answered)
    }

    #[tokio::test]
    async fn socks5_reads_ipv4_ipv6_and_domain_targets() {
        let targets: [(&[u8], &str); 3] = [
            (&[1, 10, 0, 0, 7], "10.0.0.7"),
            (&[4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], "::1"),
            (
                &[
                    3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'o', b'r', b'g',
                ],
                "example.org",
            ),
        ];
        for (address, host) in targets {
            let mut request = vec![5, 1, 0, 5, 1, 0];
            request.extend_from_slice(address);
            request.extend_from_slice(&443u16.to_be_bytes());
            let (parsed, answered) = parse(&request).await;
            let expected = SocksRequest {
                version: 5,
                host: host.into(),
                port: 443,
            };
            assert_eq!(parsed, Ok(expected));
            assert_eq!(
                answered,
                [5, 0],
                "only the method choice is sent before connecting"
            );
        }
    }

    #[tokio::test]
    async fn socks5_without_the_no_auth_method_is_refused() {
        let (parsed, answered) = parse(&[5, 1, 2]).await;
        assert!(parsed.unwrap_err().contains("authentication"));
        assert_eq!(answered, [5, 0xFF]);
    }

    #[tokio::test]
    async fn socks5_commands_other_than_connect_are_refused() {
        // BIND to 127.0.0.1:80.
        let (parsed, answered) = parse(&[5, 1, 0, 5, 2, 0, 1, 127, 0, 0, 1, 0, 80]).await;
        assert!(parsed.unwrap_err().contains("command 2"));
        assert_eq!(answered, [5, 0, 5, 7, 0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn socks4_reads_ip_targets_and_socks4a_reads_names() {
        let (parsed, answered) = parse(&[4, 1, 0, 80, 192, 168, 1, 2, b'u', 0]).await;
        let expected = SocksRequest {
            version: 4,
            host: "192.168.1.2".into(),
            port: 80,
        };
        assert_eq!(parsed, Ok(expected));
        assert!(answered.is_empty());

        let (parsed, _) = parse(&[4, 1, 0, 80, 0, 0, 0, 1, 0, b'h', b'o', b's', b't', 0]).await;
        assert_eq!(parsed.unwrap().host, "host");
    }

    #[tokio::test]
    async fn unknown_socks_versions_are_refused() {
        let (parsed, answered) = parse(b"GET / HTTP/1.1\r\n").await;
        assert!(parsed.unwrap_err().contains("version 71"));
        assert!(answered.is_empty());
    }
}
