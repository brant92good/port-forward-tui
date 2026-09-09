//! A first mutation must return EOF to a capturing shell while its daemon lives.
#![cfg(windows)]

use port_forward_tui::{background, machines::Catalog};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn capture(directory: &Path, args: &[&str]) -> (bool, bool, bool, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ports"))
        .arg("--data-dir")
        .arg(directory)
        .arg("--json")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    for (name, mut stream) in [
        (
            "stdout",
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        ),
        (
            "stderr",
            Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
        ),
    ] {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.read_to_end(&mut bytes);
            let _ = sender.send((name, result, bytes));
        });
    }
    drop(sender);
    let deadline = Instant::now() + Duration::from_secs(10);
    let exited = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.success();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    // EOF is checked before shutting down the daemon. Reading directly here
    // would hang the regression itself on the bug it is meant to detect.
    let mut captured = Vec::new();
    for _ in 0..2 {
        if let Ok(value) = receiver.recv_timeout(Duration::from_millis(500)) {
            captured.push(value);
        }
    }
    let eof = captured.len() == 2 && captured.iter().all(|(_, result, _)| result.is_ok());
    let stdin_closed = input.write_all(b"no inherited reader").is_err();
    let output = captured
        .iter()
        .find(|(name, _, _)| *name == "stdout")
        .and_then(|(_, _, bytes)| serde_json::from_slice(bytes).ok())
        .unwrap_or(Value::Null);
    (exited, eof, stdin_closed, output)
}

#[test]
#[ignore = "Run this test executable directly: Cargo owns a non-breakaway Windows job"]
fn first_save_and_restart_release_captured_stdio_while_controller_stays_alive() {
    let temporary = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temporary.path()).unwrap();
    let machine = catalog
        .add("capture-fixture.invalid", "Capture fixture", None, None)
        .unwrap();
    let first = capture(
        temporary.path(),
        &[
            "--machine",
            &machine.id,
            "save",
            "--remote",
            "18763",
            "--name",
            "Capture",
        ],
    );
    let first_live = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    );
    let restart = if first.0 && first.1 && first.2 {
        Some(capture(
            temporary.path(),
            &["--machine", &machine.id, "restart-manager"],
        ))
    } else {
        None
    };
    let final_live = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    );
    let cleanup = background::exchange(
        &machine.directory,
        "shutdown",
        json!({}),
        Duration::from_secs(2),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while machine.directory.join("endpoint.json").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        cleanup.is_ok() && !machine.directory.join("endpoint.json").exists(),
        "owned daemon cleanup failed; first={first:?}; first_live={first_live:?}; cleanup={cleanup:?}"
    );
    assert!(
        first_live.is_ok() && final_live.is_ok(),
        "daemon must remain live after each CLI exits"
    );
    assert!(first.0, "first save failed");
    assert!(
        first.1,
        "first save exited but its daemon retained captured stdout/stderr"
    );
    assert!(first.2, "first save's daemon retained captured stdin");
    assert_eq!(first.3["ok"], true);
    let restart = restart.unwrap();
    assert!(
        restart.0 && restart.1 && restart.2,
        "restart must release all incoming standard handles"
    );
    assert_eq!(restart.3["ok"], true);
    assert_ne!(first_live.unwrap()["pid"], final_live.unwrap()["pid"]);
}
