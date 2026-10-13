use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, client};
use tokio::sync::mpsc;

use crate::known_hosts::{HostKeyStatus, KnownHosts};
use crate::{Error, HostKeyInfo, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

pub enum Credentials {
    Password(String),
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
}

pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credentials: Credentials,
    pub known_hosts: KnownHosts,
}

/// An authenticated SSH connection, consumed by opening a shell or an SFTP client.
pub struct Connection {
    pub(crate) session: client::Handle<ClientHandler>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// Terminal output, stdout and stderr interleaved as the server sent them.
    Data(Vec<u8>),
    /// Always the last event of a session.
    Closed {
        exit_status: Option<u32>,
        error: Option<String>,
    },
}

enum Command {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

/// Controls a running shell. Cheap to clone; the session ends on [`close`](Self::close)
/// or when the server closes it, after which every call returns [`Error::SessionClosed`].
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl SessionHandle {
    pub fn write(&self, data: Vec<u8>) -> Result<()> {
        self.send(Command::Write(data))
    }

    pub fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        self.send(Command::Resize { cols, rows })
    }

    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| Error::SessionClosed)
    }
}

/// Connects, verifies the host key and authenticates.
pub async fn connect(request: ConnectRequest) -> Result<Connection> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        ..Default::default()
    });
    let handler = ClientHandler {
        host: request.host.clone(),
        port: request.port,
        known_hosts: request.known_hosts,
    };
    let address = (request.host.as_str(), request.port);
    let mut session =
        tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, address, handler))
            .await
            .map_err(|_| Error::Timeout)??;

    authenticate(&mut session, &request.username, request.credentials).await?;
    Ok(Connection { session })
}

impl Connection {
    /// Starts a login shell on a PTY. Output and the final close are delivered through `on_event`.
    pub async fn open_shell<F>(self, cols: u32, rows: u32, on_event: F) -> Result<SessionHandle>
    where
        F: Fn(SessionEvent) + Send + Sync + 'static,
    {
        let channel = self.session.channel_open_session().await?;
        channel
            .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
            .await?;
        channel.request_shell(false).await?;

        let (commands, receiver) = mpsc::unbounded_channel();
        tokio::spawn(run_session(self.session, channel, receiver, on_event));
        Ok(SessionHandle { commands })
    }
}

pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: KnownHosts,
}

impl client::Handler for ClientHandler {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(Error::UnsupportedHostCertificate);
        };
        match self.known_hosts.check(&self.host, self.port, key)? {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Unknown => Err(Error::UnknownHostKey(Box::new(HostKeyInfo::new(
                &self.host, self.port, key,
            )?))),
            HostKeyStatus::Changed { file, line } => Err(Error::HostKeyChanged {
                info: Box::new(HostKeyInfo::new(&self.host, self.port, key)?),
                file,
                line,
            }),
        }
    }
}

async fn authenticate(
    session: &mut client::Handle<ClientHandler>,
    user: &str,
    credentials: Credentials,
) -> Result<()> {
    let accepted = match credentials {
        Credentials::Password(password) => session
            .authenticate_password(user, password)
            .await?
            .success(),
        Credentials::Key { path, passphrase } => {
            let key = keys::load_secret_key(&path, passphrase.as_deref()).map_err(|e| match e {
                keys::Error::KeyIsEncrypted => Error::PassphraseRequired,
                e => e.into(),
            })?;
            let rsa_hash = session.best_supported_rsa_hash().await?.flatten();
            session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash))
                .await?
                .success()
        }
        Credentials::Agent => authenticate_with_agent(session, user).await?,
    };
    if accepted {
        Ok(())
    } else {
        Err(Error::AuthFailed)
    }
}

type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// The user's SSH agent: `SSH_AUTH_SOCK` on Unix; on Windows the OpenSSH
/// agent's named pipe, falling back to Pageant.
async fn connect_agent() -> Result<Agent, keys::Error> {
    #[cfg(unix)]
    {
        Ok(AgentClient::connect_env().await?.dynamic())
    }
    #[cfg(windows)]
    {
        match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
            Ok(agent) => Ok(agent.dynamic()),
            Err(_) => Ok(AgentClient::connect_pageant().await?.dynamic()),
        }
    }
}

/// Offers each key held by the user's SSH agent until the server accepts one.
async fn authenticate_with_agent(
    session: &mut client::Handle<ClientHandler>,
    user: &str,
) -> Result<bool> {
    let agent_error = |e: &dyn std::fmt::Display| Error::Agent(e.to_string());
    let mut agent = connect_agent().await.map_err(|e| agent_error(&e))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| agent_error(&e))?;
    if identities.is_empty() {
        return Err(Error::Agent(
            "the agent has no keys loaded (add one with ssh-add)".into(),
        ));
    }
    let rsa_hash = session.best_supported_rsa_hash().await?.flatten();
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        let hash_alg = if key.algorithm().is_rsa() {
            rsa_hash
        } else {
            None
        };
        let result = session
            .authenticate_publickey_with(user, key, hash_alg, &mut agent)
            .await
            .map_err(|e| agent_error(&e))?;
        if result.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn run_session<F>(
    session: client::Handle<ClientHandler>,
    channel: russh::Channel<client::Msg>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    on_event: F,
) where
    F: Fn(SessionEvent) + Send + Sync + 'static,
{
    let (mut reader, writer) = channel.split();
    let mut exit_status = None;
    let error = loop {
        tokio::select! {
            message = reader.wait() => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    on_event(SessionEvent::Data(data.to_vec()));
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(ChannelMsg::Close) | None => break None,
                Some(_) => {}
            },
            command = commands.recv() => {
                let result = match command {
                    Some(Command::Write(data)) => writer.data_bytes(data).await,
                    Some(Command::Resize { cols, rows }) => writer.window_change(cols, rows, 0, 0).await,
                    Some(Command::Close) | None => break None,
                };
                if let Err(e) = result {
                    break Some(e.to_string());
                }
            }
        }
    };
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    on_event(SessionEvent::Closed { exit_status, error });
}
