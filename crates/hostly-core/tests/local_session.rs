//! End-to-end tests of local terminals, running real programs in a real PTY.
//!
//! Unix only, since the programs are POSIX shell commands. The Windows code
//! path is the same apart from portable-pty's ConPTY backend.
#![cfg(unix)]

mod events;

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use hostly_core::Error;
use hostly_core::pty::{self, LocalCommand};
use hostly_core::session::{SessionEvent, SessionHandle};

fn sh(script: &str) -> LocalCommand {
    LocalCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        ..LocalCommand::default()
    }
}

fn spawn(command: LocalCommand) -> (SessionHandle, events::Events) {
    let (on_event, events) = events::sink();
    let handle = pty::spawn(command, 80, 24, on_event).unwrap();
    (handle, events)
}

fn closed(exit_status: Option<u32>) -> SessionEvent {
    SessionEvent::Closed {
        exit_status,
        error: None,
    }
}

#[test]
fn output_arrives_before_the_exit_status() {
    let (_handle, events) = spawn(sh("printf hi; exit 3"));
    events.output_until("hi");
    assert_eq!(events.closed(), closed(Some(3)));
    events.assert_finished();
}

#[test]
fn a_background_job_does_not_keep_the_session_open() {
    // The job holds the terminal open after the shell exits.
    let (_handle, events) = spawn(sh("sleep 3 & exit 4"));
    let started = Instant::now();
    assert_eq!(events.closed(), closed(Some(4)));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn input_round_trips_through_the_program() {
    let (handle, events) = spawn(LocalCommand {
        program: "cat".into(),
        ..LocalCommand::default()
    });
    handle.write(b"ping\n".to_vec()).unwrap();
    // The terminal echoes the line, then cat prints it back.
    events.output_until("ping\r\nping\r\n");
    // Ctrl+D: end of input, so cat exits cleanly.
    handle.write(vec![0x04]).unwrap();
    assert_eq!(events.closed(), closed(Some(0)));
    assert!(matches!(
        handle.write(b"x".to_vec()),
        Err(Error::SessionClosed)
    ));
    events.assert_finished();
}

#[test]
fn resize_reaches_the_program() {
    let (handle, events) = spawn(sh("stty size; read line; stty size"));
    events.output_until("24 80");
    handle.resize(100, 30).unwrap();
    handle.write(b"\n".to_vec()).unwrap();
    events.output_until("30 100");
    assert_eq!(events.closed(), closed(Some(0)));
}

#[test]
fn environment_and_working_directory_are_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = sh("printf '%s in ' \"$HOSTLY_TEST\"; pwd");
    command.env.insert("HOSTLY_TEST".into(), "set".into());
    command.cwd = Some(dir.path().to_path_buf());
    let (_handle, events) = spawn(command);

    let name = dir.path().file_name().unwrap().to_str().unwrap();
    let output = events.output_until(name);
    assert!(output.contains("set in /"), "{output:?}");
    assert_eq!(events.closed(), closed(Some(0)));
}

/// Starts a program that runs until it's killed, and returns its process id.
fn spawn_sleeper() -> (SessionHandle, events::Events, String) {
    let (handle, events) = spawn(sh("echo \"pid $$ ready\"; exec sleep 30"));
    let output = events.output_until(" ready");
    let pid = output.split_whitespace().nth(1).unwrap().to_owned();
    (handle, events, pid)
}

fn wait_until_gone(pid: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        assert!(Instant::now() < deadline, "process {pid} is still running");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn close_ends_the_program() {
    let (handle, events, pid) = spawn_sleeper();
    handle.close();
    assert_eq!(events.closed(), closed(None));
    assert!(matches!(
        handle.write(b"x".to_vec()),
        Err(Error::SessionClosed)
    ));
    wait_until_gone(&pid);
    events.assert_finished();
}

#[test]
fn dropping_every_handle_ends_the_program() {
    let (handle, events, pid) = spawn_sleeper();
    drop(handle);
    assert_eq!(events.closed(), closed(None));
    wait_until_gone(&pid);
}

#[test]
fn a_program_that_cannot_start_is_a_pty_error() {
    let command = LocalCommand {
        program: "/nonexistent/hostly-test".into(),
        ..LocalCommand::default()
    };
    let error = pty::spawn(command, 80, 24, |_| {}).unwrap_err();
    assert_eq!(error.kind(), "pty");
    assert!(error.to_string().contains("/nonexistent/hostly-test"));
}
