//! A first mutation must return EOF to a capturing shell while its daemon lives.
#![cfg(windows)]

use port_forward_tui::{background, machines::Catalog, process::spawn_daemon};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation,
    },
    System::Pipes::{CreatePipe, PeekNamedPipe},
};

// The sentinel is intentionally inheritable. Keep fixture spawns serial so a
// sibling test's short-lived CLI cannot inherit it and confuse the EOF oracle.
static CAPTURE_SPAWNS: Mutex<()> = Mutex::new(());

/// Shut down only the controller authenticated by this disposable directory.
/// This also runs if a regression assertion fails before explicit cleanup.
struct ControllerCleanup(PathBuf);
impl ControllerCleanup {
    fn shutdown(&self) -> bool {
        if !self.0.join("endpoint.json").exists() {
            return true;
        }
        let result = background::exchange(&self.0, "shutdown", json!({}), Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.join("endpoint.json").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        result.is_ok() && !self.0.join("endpoint.json").exists()
    }
}
impl Drop for ControllerCleanup {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn trailing_separator(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push("\\");
    PathBuf::from(value)
}

fn sentinel_pipe() -> (OwnedHandle, OwnedHandle) {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    assert_ne!(
        unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) },
        0
    );
    let read = unsafe { OwnedHandle::from_raw_handle(read) };
    let write = unsafe { OwnedHandle::from_raw_handle(write) };
    assert_ne!(
        unsafe {
            SetHandleInformation(
                write.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        0
    );
    let mut flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(write.as_raw_handle(), &mut flags) },
        0
    );
    assert_ne!(
        flags & HANDLE_FLAG_INHERIT,
        0,
        "Sentinel must actually be inheritable"
    );
    (read, write)
}

fn pipe_eof(pipe: &OwnedHandle) -> io::Result<bool> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                Ok(true)
            } else {
                Err(error)
            };
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

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
    let _serial = CAPTURE_SPAWNS.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let directory = trailing_separator(
        &temporary
            .path()
            .join("\u{81fa}\u{4e2d} \u{1f680}'s capture"),
    );
    let catalog = Catalog::new(&directory).unwrap();
    let machine = catalog
        .add("capture-fixture.invalid", "Capture fixture", None, None)
        .unwrap();
    let _cleanup = ControllerCleanup(machine.directory.clone());
    let first = capture(
        &directory,
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
            &directory,
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

#[test]
#[ignore = "Run this test executable directly: Cargo owns a non-breakaway Windows job"]
fn daemon_excludes_unrelated_inheritable_pipe_and_preserves_exact_paths() {
    let _serial = CAPTURE_SPAWNS.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("\u{81fa}\u{4e2d} \u{1f680}'s daemon");
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("ports.exe");
    fs::copy(env!("CARGO_BIN_EXE_ports"), &executable).unwrap();
    let machine = Catalog::new(&root)
        .unwrap()
        .add("sentinel-fixture.invalid", "Exact path fixture", None, None)
        .unwrap();
    let cleanup = ControllerCleanup(machine.directory.clone());
    let directory = trailing_separator(&machine.directory);
    let log = File::create(root.join("daemon.log")).unwrap();

    // This handle is deliberately unrelated to stdin/stdout/stderr. Clearing
    // only the three incoming standard handles cannot make this test pass.
    let (read, write) = sentinel_pipe();
    let mut child = spawn_daemon(&executable, &directory, log).unwrap();
    drop(write);
    let eof_while_alive = pipe_eof(&read);
    let deadline = Instant::now() + Duration::from_secs(8);
    let live = loop {
        let result = background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_millis(250),
        );
        if result.is_ok() || Instant::now() >= deadline {
            break result;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let running = child.try_wait().unwrap().is_none();
    // Drop only the process handle, then prove that the daemon still serves the
    // exact Unicode/apostrophe/spaced directory passed with a trailing slash.
    drop(child);
    let after_handle_drop = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    );
    let cleaned = cleanup.shutdown();
    assert!(cleaned, "Owned sentinel daemon did not shut down");
    assert!(
        live.is_ok() && after_handle_drop.is_ok() && running,
        "Exact-path daemon did not remain live: {live:?}; {after_handle_drop:?}"
    );
    assert!(
        matches!(eof_while_alive, Ok(true)),
        "Daemon inherited a non-stdio sentinel pipe: {eof_while_alive:?}"
    );
}

#[test]
fn public_daemon_spawn_rejects_nul_before_process_creation() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = Path::new(env!("CARGO_BIN_EXE_ports"));
    let mut nul_directory = temporary.path().as_os_str().to_os_string();
    nul_directory.push(OsString::from("\0ignored"));
    let mut nul_executable = executable.as_os_str().to_os_string();
    nul_executable.push(OsString::from("\0ignored"));
    for (exe, directory) in [
        (executable, Path::new(&nul_directory)),
        (Path::new(&nul_executable), temporary.path()),
    ] {
        let result = spawn_daemon(exe, directory, tempfile::tempfile().unwrap());
        assert!(
            result.is_err(),
            "NUL must be rejected before spawning a daemon"
        );
        assert!(result.err().unwrap().to_string().contains("NUL"));
    }
    assert!(!temporary.path().join("endpoint.json").exists());
}
