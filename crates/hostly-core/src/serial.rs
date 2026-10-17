//! Serial connections, with the same event contract as SSH shells.

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};
use tokio::sync::mpsc::{self, WeakUnboundedSender};

use crate::session::{Command, Emitter, SessionEvent, SessionHandle};
use crate::{Error, Result};

/// How long a read or write waits before checking whether the session was closed.
/// Windows runs one read or write at a time per port, so there a write may wait
/// for a read to time out, which keeps this short.
const POLL_INTERVAL: Duration = if cfg!(windows) {
    Duration::from_millis(10)
} else {
    Duration::from_millis(100)
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialPortInfo {
    /// What [`open`] takes: a device path such as `/dev/cu.usbserial-0001`, or `COM3`.
    pub name: String,
    /// The USB product or manufacturer, or the kind of port, when known.
    pub description: Option<String>,
}

/// The serial ports present, sorted by name.
pub fn list_ports() -> Result<Vec<SerialPortInfo>> {
    // Built without libudev, the Linux scan reads sysfs, and panics when it's
    // missing (as in some containers).
    if cfg!(target_os = "linux") && !std::path::Path::new("/sys/class/tty").is_dir() {
        return Ok(Vec::new());
    }
    let found = serialport::available_ports()
        .map_err(|e| Error::Serial(format!("couldn't list serial ports: {e}")))?;
    let names: HashSet<String> = found.iter().map(|port| port.port_name.clone()).collect();
    let mut ports: Vec<SerialPortInfo> = found
        .into_iter()
        // macOS lists each port twice: /dev/tty.* is for dialing in, /dev/cu.* for connecting out.
        .filter(|port| {
            port.port_name
                .strip_prefix("/dev/tty.")
                .is_none_or(|device| !names.contains(&format!("/dev/cu.{device}")))
        })
        .map(|port| SerialPortInfo {
            description: description(&port.port_type),
            name: port.port_name,
        })
        .collect();
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    ports.dedup_by(|a, b| a.name == b.name);
    Ok(ports)
}

fn description(port_type: &SerialPortType) -> Option<String> {
    match port_type {
        SerialPortType::UsbPort(usb) => Some(
            usb.product
                .clone()
                .or_else(|| usb.manufacturer.clone())
                .unwrap_or_else(|| format!("USB device {:04x}:{:04x}", usb.vid, usb.pid)),
        ),
        SerialPortType::PciPort => Some("PCI serial port".into()),
        SerialPortType::BluetoothPort => Some("Bluetooth".into()),
        SerialPortType::Unknown => None,
    }
}

/// Opens the port at `path` with 8 data bits, no parity, one stop bit and no
/// flow control.
///
/// Received bytes and then exactly one `Closed` are delivered through
/// `on_event`. `Closed` carries an error when the device went away, as when
/// it's unplugged. Resizing does nothing, and the port is closed by the time
/// `Closed` is delivered. The blocking I/O runs on threads of its own.
pub fn open<F>(path: &str, baud_rate: u32, on_event: F) -> Result<SessionHandle>
where
    F: Fn(SessionEvent) + Send + Sync + 'static,
{
    let port = serialport::new(path, baud_rate)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(POLL_INTERVAL)
        .open()
        .map_err(|e| open_error(path, &e))?;
    let reader = port
        .try_clone()
        .map_err(|e| Error::Serial(format!("couldn't use {path}: {e}")))?;

    let (commands, mut receiver) = mpsc::unbounded_channel();
    let session = Arc::new(Session {
        events: Emitter::new(on_event),
        stop: AtomicBool::new(false),
        failure: Mutex::new(None),
        // Weak, so that dropping every handle still ends the session.
        wake: commands.downgrade(),
    });
    let (input, input_queue) = channel();

    let reader_session = session.clone();
    let reading = thread::spawn(move || read_input(reader, &reader_session));
    let writer_session = session.clone();
    let writing = thread::spawn(move || write_output(port, input_queue, &writer_session));
    thread::spawn(move || {
        while let Some(command) = receiver.blocking_recv() {
            match command {
                Command::Write(data) => {
                    let _ = input.send(data);
                }
                Command::Resize { .. } => {}
                Command::Close => break,
            }
        }
        receiver.close();
        session.stop.store(true, Ordering::Relaxed);
        drop(input);
        // Each thread notices the stop within a poll interval, closing its handle to the port.
        let _ = reading.join();
        let _ = writing.join();
        let failure = session.failure.lock().unwrap().take();
        session.events.close(None, failure);
    });

    Ok(SessionHandle { commands })
}

struct Session<F> {
    events: Emitter<F>,
    /// Set when the session closes, for the reading and writing threads.
    stop: AtomicBool,
    /// Why the port stopped working, if it did.
    failure: Mutex<Option<String>>,
    wake: WeakUnboundedSender<Command>,
}

impl<F> Session<F> {
    /// Records why the port stopped working, and has the session close.
    fn fail(&self, error: String) {
        self.failure.lock().unwrap().get_or_insert(error);
        if let Some(commands) = self.wake.upgrade() {
            let _ = commands.send(Command::Close);
        }
    }
}

/// Is `error` only a read or write that waited a poll interval without finishing?
fn waited(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::TimedOut | ErrorKind::Interrupted | ErrorKind::WouldBlock
    )
}

fn read_input<F: Fn(SessionEvent)>(mut port: Box<dyn SerialPort>, session: &Session<F>) {
    let mut buffer = [0; 4096];
    while !session.stop.load(Ordering::Relaxed) {
        match port.read(&mut buffer) {
            Ok(0) => return session.fail("the device was disconnected".into()),
            Ok(read) => {
                session.events.data(&buffer[..read]);
            }
            Err(e) if waited(&e) => {}
            Err(e) => return session.fail(format!("the device was disconnected ({e})")),
        }
    }
}

fn write_output<F>(mut port: Box<dyn SerialPort>, queue: Receiver<Vec<u8>>, session: &Session<F>) {
    for data in queue {
        let mut rest = data.as_slice();
        while !rest.is_empty() {
            if session.stop.load(Ordering::Relaxed) {
                return;
            }
            match port.write(rest) {
                // Windows reports a write that timed out as 0 bytes written.
                Ok(written) => rest = &rest[written..],
                Err(e) if waited(&e) => {}
                Err(e) => return session.fail(format!("couldn't send to the device ({e})")),
            }
        }
    }
}

fn open_error(path: &str, error: &serialport::Error) -> Error {
    let hint = match error.kind() {
        serialport::ErrorKind::Io(ErrorKind::PermissionDenied) if cfg!(target_os = "linux") => {
            "; on Linux, serial ports usually need membership in the dialout group"
        }
        // On Unix that's a busy port; Windows also uses it for missing ones.
        serialport::ErrorKind::NoDevice if cfg!(unix) => "; another program is using it",
        serialport::ErrorKind::NoDevice => "; it may be in use by another program",
        _ => "",
    };
    Error::Serial(format!("couldn't open {path}: {error}{hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::UsbPortInfo;

    fn usb(product: Option<&str>, manufacturer: Option<&str>) -> SerialPortType {
        SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x10c4,
            pid: 0xea60,
            serial_number: None,
            manufacturer: manufacturer.map(Into::into),
            product: product.map(Into::into),
        })
    }

    #[test]
    fn ports_are_described_by_what_is_known_about_them() {
        let cp2102 = "CP2102 USB to UART Bridge Controller";
        assert_eq!(
            description(&usb(Some(cp2102), Some("Silicon Labs"))).as_deref(),
            Some(cp2102)
        );
        assert_eq!(
            description(&usb(None, Some("Silicon Labs"))).as_deref(),
            Some("Silicon Labs")
        );
        assert_eq!(
            description(&usb(None, None)).as_deref(),
            Some("USB device 10c4:ea60")
        );
        assert_eq!(description(&SerialPortType::Unknown), None);
    }

    #[test]
    fn listing_ports_works_on_this_machine() {
        let ports = list_ports().unwrap();
        let mut names: Vec<_> = ports.iter().map(|port| &port.name).collect();
        names.dedup();
        assert_eq!(names.len(), ports.len());
    }

    #[test]
    fn port_info_serializes_in_camel_case() {
        let info = SerialPortInfo {
            name: "COM3".into(),
            description: None,
        };
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"name":"COM3","description":null}"#
        );
    }

    #[test]
    fn a_missing_port_is_a_serial_error() {
        let error = open("/dev/hostly-no-such-port", 9600, |_| {}).unwrap_err();
        assert_eq!(error.kind(), "serial");
        assert!(error.to_string().contains("/dev/hostly-no-such-port"));
    }
}
