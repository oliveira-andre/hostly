//! Collects the events of a session that runs on its own threads (local
//! terminals, serial ports, Mosh), for tests that don't need async.
#![allow(dead_code)] // Each test binary uses a different part of this module.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use hostly_core::session::SessionEvent;

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Events(mpsc::Receiver<SessionEvent>);

/// An `on_event` callback, and what it receives.
pub fn sink() -> (impl Fn(SessionEvent) + Send + Sync + 'static, Events) {
    let (sender, receiver) = mpsc::channel();
    let on_event = move |event| {
        let _ = sender.send(event);
    };
    (on_event, Events(receiver))
}

impl Events {
    pub fn next(&self) -> SessionEvent {
        self.0
            .recv_timeout(TIMEOUT)
            .expect("no session event arrived")
    }

    /// Collects output until it contains `needle`.
    pub fn output_until(&self, needle: &str) -> String {
        let mut output = String::new();
        while !output.contains(needle) {
            match self.next() {
                SessionEvent::Data(bytes) => output.push_str(&String::from_utf8_lossy(&bytes)),
                closed => panic!(
                    "session closed before {needle:?} arrived: {closed:?} (output: {output:?})"
                ),
            }
        }
        output
    }

    pub fn closed(&self) -> SessionEvent {
        loop {
            if let event @ SessionEvent::Closed { .. } = self.next() {
                return event;
            }
        }
    }

    /// Waits for the session to drop its callback, which happens once all its
    /// threads have ended, and checks that nothing came after `Closed`.
    pub fn assert_finished(&self) {
        match self.0.recv_timeout(TIMEOUT) {
            Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => panic!("the session's threads are still running"),
            Ok(event) => panic!("an event came after Closed: {event:?}"),
        }
    }
}
