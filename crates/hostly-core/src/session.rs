//! What every interactive session has in common, whether it's an SSH shell,
//! a local terminal, a serial port or Mosh: the events it reports and the
//! handle that drives it.

use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::{Error, Result};

#[derive(Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// Terminal output: raw bytes, as the server, program or device sent them.
    Data(Vec<u8>),
    /// Always the last event of a session.
    Closed {
        exit_status: Option<u32>,
        error: Option<String>,
    },
}

pub(crate) enum Command {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

/// Controls a running session. Cheap to clone; the session ends on [`close`](Self::close),
/// when the other side ends it, or once every handle is dropped. After that every call
/// returns [`Error::SessionClosed`].
#[derive(Clone, Debug)]
pub struct SessionHandle {
    pub(crate) commands: mpsc::UnboundedSender<Command>,
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

/// Delivers the events of a session that runs on several threads, and keeps
/// the contract: `Closed` comes exactly once, and nothing comes after it.
pub(crate) struct Emitter<F> {
    on_event: F,
    closed: Mutex<bool>,
}

impl<F: Fn(SessionEvent)> Emitter<F> {
    pub(crate) fn new(on_event: F) -> Self {
        Self {
            on_event,
            closed: Mutex::new(false),
        }
    }

    /// Returns `false`, sending nothing, once the session has closed.
    pub(crate) fn data(&self, bytes: &[u8]) -> bool {
        let closed = self.closed.lock().unwrap();
        if !*closed {
            (self.on_event)(SessionEvent::Data(bytes.to_vec()));
        }
        !*closed
    }

    /// Sends `Closed`, unless it was already sent.
    pub(crate) fn close(&self, exit_status: Option<u32>, error: Option<String>) {
        let mut closed = self.closed.lock().unwrap();
        if !*closed {
            *closed = true;
            (self.on_event)(SessionEvent::Closed { exit_status, error });
        }
    }
}
