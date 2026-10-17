//! End-to-end serial tests against a pseudo-terminal pair: the session opens
//! the PTY's device end as its serial port, and the test plays the device on
//! the other end.
//!
//! Unix only, since Windows has no pseudo-terminal devices to open as ports.
#![cfg(unix)]

mod events;

use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hostly_core::Error;
use hostly_core::serial;
use hostly_core::session::SessionEvent;
use serialport::{SerialPort, TTYPort};

/// macOS can't set a PTY's baud rate, and 0 tells serialport to skip it.
const BAUD_RATE: u32 = if cfg!(target_os = "macos") { 0 } else { 9600 };

/// `TTYPort::pair` isn't thread-safe on every platform.
static PAIRS: Mutex<()> = Mutex::new(());

/// A pseudo-terminal: the end the test drives, and the device path to open as a port.
fn device() -> (TTYPort, String) {
    let _guard = PAIRS.lock().unwrap();
    let (device, port) = TTYPort::pair().unwrap();
    // Only the path is needed; the session opens the port itself.
    (device, port.name().unwrap())
}

fn read_until(device: &mut TTYPort, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = Vec::new();
    let mut buffer = [0; 256];
    while !received
        .windows(needle.len())
        .any(|window| window == needle)
    {
        assert!(Instant::now() < deadline, "received only {received:?}");
        match device.read(&mut buffer) {
            Ok(read) => received.extend_from_slice(&buffer[..read]),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("reading from the session failed: {e}"),
        }
    }
    received
}

#[test]
fn bytes_flow_both_ways() {
    let (mut device, path) = device();
    let (on_event, events) = events::sink();
    let handle = serial::open(&path, BAUD_RATE, on_event).unwrap();

    device.write_all(b"login: ").unwrap();
    events.output_until("login: ");
    // Raw mode: bytes go through untouched, carriage returns included.
    handle.write(b"root\r".to_vec()).unwrap();
    read_until(&mut device, b"root\r");
    handle.resize(120, 40).unwrap();

    handle.close();
    assert_eq!(
        events.closed(),
        SessionEvent::Closed {
            exit_status: None,
            error: None
        }
    );
    assert!(matches!(
        handle.write(b"x".to_vec()),
        Err(Error::SessionClosed)
    ));
    events.assert_finished();
}

#[test]
fn close_releases_the_port() {
    let (_device, path) = device();
    let (on_event, events) = events::sink();
    let handle = serial::open(&path, BAUD_RATE, on_event).unwrap();
    // Ports are opened exclusively, so a second open fails while the first is open.
    let second = serial::open(&path, BAUD_RATE, |_| {}).unwrap_err();
    assert_eq!(second.kind(), "serial");

    handle.close();
    events.closed();
    let reopened = serial::open(&path, BAUD_RATE, |_| {}).unwrap();
    reopened.close();
}

#[test]
fn a_vanished_device_ends_the_session_with_an_error() {
    let (device, path) = device();
    let (on_event, events) = events::sink();
    let _handle = serial::open(&path, BAUD_RATE, on_event).unwrap();

    // Closing the other end of the PTY is what unplugging does to a real port.
    drop(device);
    let SessionEvent::Closed { exit_status, error } = events.closed() else {
        unreachable!()
    };
    assert_eq!(exit_status, None);
    assert!(error.is_some_and(|error| error.contains("disconnected")));
    events.assert_finished();
}
