//! An in-process SSH server for end-to-end tests: an echo shell, an SFTP
//! subsystem serving the real filesystem, TCP port forwarding both ways, and
//! a stand-in for `mosh-server`.
#![allow(dead_code)] // Each test binary uses a different part of this module.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hostly_core::known_hosts::KnownHosts;
use hostly_core::sftp::SftpClient;
use hostly_core::ssh::{self, ConnectRequest, Credentials};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure, Disconnect, Pty};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub const USER: &str = "tester";
pub const PASSWORD: &str = "s3cret";
/// What `mosh-server` prints on the test server.
pub const MOSH_PORT: u16 = 60001;
pub const MOSH_KEY: &str = "AbCdEfGhIjKlMnOpQrStUv";

pub struct TestServer {
    pub port: u16,
    pub host_key: PublicKey,
    pub known_hosts: KnownHosts,
    /// What the SFTP subsystem reports as the login directory (canonicalized).
    pub sftp_root: PathBuf,
    /// Every authenticated client, so tests can drop them.
    clients: Arc<Mutex<Vec<server::Handle>>>,
    /// Cleared, `mosh-server` fails as a shell does when it isn't installed.
    pub mosh_installed: Arc<AtomicBool>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

pub async fn start_server() -> TestServer {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let host_key = key.public_key().clone();
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let root_dir = tempfile::tempdir().unwrap();
    let sftp_root = fs::canonicalize(root_dir.path()).unwrap();
    let clients = Arc::new(Mutex::new(Vec::new()));
    let mosh_installed = Arc::new(AtomicBool::new(true));
    let mut factory = TestSsh {
        sftp_root: sftp_root.clone(),
        clients: clients.clone(),
        mosh_installed: mosh_installed.clone(),
    };
    tokio::spawn(async move { factory.run_on_socket(config, &listener).await });

    let client_dir = tempfile::tempdir().unwrap();
    let known_hosts = KnownHosts {
        user_file: Some(client_dir.path().join("user_known_hosts")),
        app_file: client_dir.path().join("app_known_hosts"),
    };
    TestServer {
        port,
        host_key,
        known_hosts,
        sftp_root,
        clients,
        mosh_installed,
        _dirs: (root_dir, client_dir),
    }
}

impl TestServer {
    pub fn request(&self, password: &str) -> ConnectRequest {
        ConnectRequest {
            host: "127.0.0.1".into(),
            port: self.port,
            username: USER.into(),
            credentials: Credentials::Password(password.into()),
            known_hosts: self.known_hosts.clone(),
        }
    }

    pub fn trust_server(&self) {
        self.known_hosts
            .trust("127.0.0.1", self.port, &self.host_key)
            .unwrap();
    }

    pub async fn sftp(&self) -> SftpClient {
        self.trust_server();
        ssh::connect(self.request(PASSWORD))
            .await
            .unwrap()
            .open_sftp()
            .await
            .unwrap()
    }

    /// Disconnects every client with `reason`, as a server shutting down would.
    pub async fn disconnect_clients(&self, reason: &str) {
        let clients = std::mem::take(&mut *self.clients.lock().unwrap());
        for client in clients {
            let _ = client
                .disconnect(Disconnect::ByApplication, reason.into(), "en".into())
                .await;
        }
    }
}

struct TestSsh {
    sftp_root: PathBuf,
    clients: Arc<Mutex<Vec<server::Handle>>>,
    mosh_installed: Arc<AtomicBool>,
}

impl server::Server for TestSsh {
    type Handler = TestConnection;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> TestConnection {
        TestConnection {
            sftp_root: self.sftp_root.clone(),
            mosh_installed: self.mosh_installed.clone(),
            channels: HashMap::new(),
            session_channels: HashSet::new(),
            sftp_channels: HashSet::new(),
            remote_forwards: HashMap::new(),
            clients: self.clients.clone(),
        }
    }
}

/// Echoes shell input, reports PTY and resize requests as output, exits with
/// status 7 on `exit\r`, and serves SFTP from the real filesystem. Connects
/// `direct-tcpip` channels to their destination, listens for `tcpip-forward`,
/// and answers `mosh-server` commands.
struct TestConnection {
    sftp_root: PathBuf,
    mosh_installed: Arc<AtomicBool>,
    // Held until the client picks a shell (dropped) or SFTP (turned into a stream).
    channels: HashMap<ChannelId, Channel<Msg>>,
    // `data()` echoes only these: russh passes it tunnel data too.
    session_channels: HashSet<ChannelId>,
    // russh also passes their data to `data()`, which must not echo it.
    sftp_channels: HashSet<ChannelId>,
    // Listening for the client's `tcpip-forward` requests, by address and port.
    remote_forwards: HashMap<(String, u32), AbortOnDrop>,
    clients: Arc<Mutex<Vec<server::Handle>>>,
}

/// Stops a task when the connection that started it goes away.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn pump(mut socket: TcpStream, channel: Channel<Msg>) {
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut channel.into_stream()).await;
}

impl server::Handler for TestConnection {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        self.clients.lock().unwrap().push(session.handle());
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let destination = (host_to_connect.to_owned(), port_to_connect as u16);
        tokio::spawn(async move {
            match TcpStream::connect(destination).await {
                Ok(socket) => {
                    reply.accept().await;
                    pump(socket, channel).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Ok(listener) = TcpListener::bind((address, *port as u16)).await else {
            return Ok(false);
        };
        *port = listener.local_addr().unwrap().port().into();
        let (handle, connected_address, connected_port) =
            (session.handle(), address.to_owned(), *port);
        let accepting = tokio::spawn(async move {
            while let Ok((socket, peer)) = listener.accept().await {
                let (handle, connected_address) = (handle.clone(), connected_address.clone());
                tokio::spawn(async move {
                    let opened = handle
                        .channel_open_forwarded_tcpip(
                            connected_address,
                            connected_port,
                            peer.ip().to_string(),
                            peer.port().into(),
                        )
                        .await;
                    if let Ok(channel) = opened {
                        pump(socket, channel).await;
                    }
                });
            }
        });
        self.remote_forwards
            .insert((address.to_owned(), *port), AbortOnDrop(accepting));
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(self
            .remote_forwards
            .remove(&(address.to_owned(), port))
            .is_some())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.session_channels.insert(channel.id());
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("pty {cols}x{rows}\r\n").into_bytes())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        session.data(channel, b"welcome\r\n".to_vec())
    }

    /// `mosh-server …` prints a `MOSH CONNECT` line and exits, as the real one
    /// does once it has detached. No other command exists.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        session.channel_success(channel)?;
        let installed = self.mosh_installed.load(Ordering::Relaxed);
        let status = if command.starts_with(b"mosh-server") && installed {
            let line = format!("\r\nMOSH CONNECT {MOSH_PORT} {MOSH_KEY}\r\n");
            session.data(channel, line.into_bytes())?;
            0
        } else {
            let name = String::from_utf8_lossy(command.split(|&b| b == b' ').next().unwrap());
            let error = format!("bash: line 1: {name}: command not found\n");
            session.extended_data(channel, 1, error.into_bytes())?;
            127
        };
        session.exit_status_request(channel, status)?;
        session.eof(channel)?;
        session.close(channel)
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.channels.remove(&channel) {
            Some(stream) if name == "sftp" => {
                self.sftp_channels.insert(channel);
                session.channel_success(channel)?;
                russh_sftp::server::run(stream.into_stream(), FsSftp::new(&self.sftp_root)).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("resized {cols}x{rows}\r\n").into_bytes())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.sftp_channels.contains(&channel) || !self.session_channels.contains(&channel) {
            Ok(())
        } else if data == b"exit\r" {
            session.exit_status_request(channel, 7)?;
            session.eof(channel)?;
            session.close(channel)
        } else {
            session.data(channel, data.to_vec())
        }
    }
}

enum OpenHandle {
    File(fs::File),
    /// Entries are sent in one batch, then the directory reports EOF.
    Dir(Option<Vec<File>>),
}

/// Just enough of an SFTP server for the client's operations, on real paths.
struct FsSftp {
    root: PathBuf,
    handles: HashMap<String, OpenHandle>,
    next_handle: u64,
}

impl FsSftp {
    fn new(root: &std::path::Path) -> Self {
        Self {
            root: root.to_path_buf(),
            handles: HashMap::new(),
            next_handle: 0,
        }
    }

    fn insert(&mut self, handle: OpenHandle) -> String {
        self.next_handle += 1;
        let id = self.next_handle.to_string();
        self.handles.insert(id.clone(), handle);
        id
    }

    fn file(&mut self, handle: &str) -> Result<&mut fs::File, StatusCode> {
        match self.handles.get_mut(handle) {
            Some(OpenHandle::File(file)) => Ok(file),
            _ => Err(StatusCode::Failure),
        }
    }
}

fn status(error: std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, StatusCode> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        let resolved = match path.as_str() {
            "." | "" => self.root.clone(),
            path => fs::canonicalize(path).map_err(status)?,
        };
        Ok(Name {
            id,
            files: vec![File::dummy(resolved.to_string_lossy())],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&path).map_err(status)? {
            let entry = entry.map_err(status)?;
            let metadata = entry.metadata().map_err(status)?;
            files.push(File::new(
                entry.file_name().to_string_lossy(),
                FileAttributes::from(&metadata),
            ));
        }
        let handle = self.insert(OpenHandle::Dir(Some(files)));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        match self.handles.get_mut(&handle) {
            Some(OpenHandle::Dir(files)) => files
                .take()
                .map(|files| Name { id, files })
                .ok_or(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let metadata = fs::metadata(path).map_err(status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let metadata = fs::symlink_metadata(path).map_err(status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        let metadata = self.file(&handle)?.metadata().map_err(status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        let file = fs::OpenOptions::from(pflags)
            .open(&filename)
            .map_err(status)?;
        let handle = self.insert(OpenHandle::File(file));
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusCode> {
        let file = self.file(&handle)?;
        file.seek(SeekFrom::Start(offset)).map_err(status)?;
        let mut data = vec![0; len as usize];
        let read = file.read(&mut data).map_err(status)?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(read);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        let file = self.file(&handle)?;
        file.seek(SeekFrom::Start(offset)).map_err(status)?;
        file.write_all(&data).map_err(status)?;
        Ok(ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, StatusCode> {
        #[cfg(unix)]
        if let Some(mode) = attrs.permissions {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777)).map_err(status)?;
        }
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        _handle: String,
        _attrs: FileAttributes,
    ) -> Result<Status, StatusCode> {
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, StatusCode> {
        fs::create_dir(path).map_err(status)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        fs::remove_dir(path).map_err(status)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusCode> {
        fs::remove_file(filename).map_err(status)?;
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, StatusCode> {
        // SFTP v3 rename never replaces an existing target.
        if fs::symlink_metadata(&newpath).is_ok() {
            return Err(StatusCode::Failure);
        }
        fs::rename(oldpath, newpath).map_err(status)?;
        Ok(ok(id))
    }
}
