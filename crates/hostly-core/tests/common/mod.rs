//! An in-process SSH server for end-to-end tests: an echo shell plus an SFTP
//! subsystem serving the real filesystem.
#![allow(dead_code)] // Each test binary uses a different part of this module.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hostly_core::known_hosts::KnownHosts;
use hostly_core::sftp::SftpClient;
use hostly_core::ssh::{self, ConnectRequest, Credentials};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId, Pty};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::net::TcpListener;

pub const USER: &str = "tester";
pub const PASSWORD: &str = "s3cret";

pub struct TestServer {
    pub port: u16,
    pub host_key: PublicKey,
    pub known_hosts: KnownHosts,
    /// What the SFTP subsystem reports as the login directory (canonicalized).
    pub sftp_root: PathBuf,
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
    let mut factory = TestSsh {
        sftp_root: sftp_root.clone(),
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
}

struct TestSsh {
    sftp_root: PathBuf,
}

impl server::Server for TestSsh {
    type Handler = TestConnection;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> TestConnection {
        TestConnection {
            sftp_root: self.sftp_root.clone(),
            channels: HashMap::new(),
            sftp_channels: HashSet::new(),
        }
    }
}

/// Echoes shell input, reports PTY and resize requests as output, exits with
/// status 7 on `exit\r`, and serves SFTP from the real filesystem.
struct TestConnection {
    sftp_root: PathBuf,
    // Held until the client picks a shell (dropped) or SFTP (turned into a stream).
    channels: HashMap<ChannelId, Channel<Msg>>,
    // russh also passes their data to `data()`, which must not echo it.
    sftp_channels: HashSet<ChannelId>,
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

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
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
        if self.sftp_channels.contains(&channel) {
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
