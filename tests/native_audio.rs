#![cfg(windows)]
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    path: std::ffi::OsString,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        fs::create_dir(&root).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let output = Command::new("rustc")
            .arg("tests/fixtures/audio_child.rs")
            .args(["--edition=2024", "-o"])
            .arg(bin.join("ssh.exe"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::copy(bin.join("ssh.exe"), bin.join("ffmpeg.exe")).unwrap();
        fs::copy(bin.join("ssh.exe"), bin.join("ffplay.exe")).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let data = root.join("data");
        let fixture = Self {
            temp,
            root,
            data,
            path,
        };
        fixture.ok(&["machines", "add", "workbox"]);
        fixture.ok(&[
            "audio",
            "configure",
            "--microphone",
            "Synthetic device",
            "--remote-script",
            "/fixture/bridge.py",
            "--expected-host",
            "fixture",
            "--ffmpeg",
            bin.join("ffmpeg.exe").to_str().unwrap(),
            "--ffplay",
            bin.join("ffplay.exe").to_str().unwrap(),
        ]);
        fixture
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ports"));
        cmd.args(["--json", "--data-dir"])
            .arg(&self.data)
            .env("PATH", &self.path)
            .env("PORTS_AUDIO_FIXTURE", &self.root);
        cmd
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "args={args:?} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn wait_state(&self, phase: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let v = self.ok(&["audio", "status"]);
            if v["last"]["phase"] == phase {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "State never reached {phase}: {v}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self
            .command()
            .args(["audio", "stop"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = &self.temp;
    }
}
fn handles(state: &Value) -> Vec<isize> {
    state["last"]["children"]
        .as_object()
        .unwrap()
        .values()
        .map(|pid| {
            let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid.as_u64().unwrap() as u32) };
            assert!(!h.is_null(), "Child not alive at capture");
            h as isize
        })
        .collect()
}
fn assert_exited(handles: Vec<isize>) {
    for h in handles {
        unsafe {
            assert_eq!(
                WaitForSingleObject(h as _, 3000),
                WAIT_OBJECT_0,
                "Owned child survived stop/death"
            );
            CloseHandle(h as _);
        }
    }
}

#[test]
fn binary_roundtrip_duplicate_stop_and_restart() {
    let f = Fixture::new();
    let started = f.ok(&["audio", "start"]);
    let children = handles(&started);
    let duplicate = f.ok(&["audio", "start"]);
    assert_eq!(started["last"]["token"], duplicate["last"]["token"]);
    let deadline = Instant::now() + Duration::from_secs(2);
    let counts = loop {
        let received = fs::read_to_string(f.root.join("received.txt")).unwrap_or_default();
        let values: Vec<usize> = received
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        if values.len() == 2 {
            assert_eq!(values[1], 0, "Binary PCM corrupted");
            if values[0] > 4096 {
                break values;
            }
        }
        assert!(
            Instant::now() < deadline,
            "No complete fixture counter record"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(counts[0] > 4096);
    assert_eq!(counts[1], 0, "Binary PCM corrupted");
    let stopped = f.ok(&["audio", "stop"]);
    assert_eq!(stopped["running"], false);
    assert_eq!(stopped["last"]["restoration"], "verified");
    assert_exited(children);
    let restart = f.ok(&["audio", "start"]);
    assert_ne!(started["last"]["token"], restart["last"]["token"]);
    f.ok(&["audio", "stop"]);
}
#[test]
fn rapid_stop_cannot_complete_before_pending_start() {
    let f = Fixture::new();
    let mut start = f
        .command()
        .env("AUDIO_SLOW_STATUS", "1")
        .args(["audio", "start"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    f.wait_state("starting");
    let stopped = f.ok(&["audio", "stop"]);
    assert!(start.wait().unwrap().success());
    assert_eq!(stopped["running"], false);
    thread::sleep(Duration::from_millis(700));
    assert_eq!(f.ok(&["audio", "status"])["running"], false);
}
#[test]
fn worker_death_kills_children_and_failed_playback_closes_capture() {
    let f = Fixture::new();
    let mut worker = f
        .command()
        .args(["audio", "run", "--token", "owned-test"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let active = f.wait_state("streaming");
    let children = handles(&active);
    worker.kill().unwrap();
    worker.wait().unwrap();
    assert_exited(children);
    assert_eq!(f.ok(&["audio", "status"])["last"]["phase"], "interrupted");
    // Fixture cleanup models reconnecting after remote EOF; no real host involved.
    let _ = fs::remove_file(f.root.join("active"));
    let mut failing_start = f
        .command()
        .env("AUDIO_EARLY_EXIT", "1")
        .args(["audio", "start"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let limit = Instant::now() + Duration::from_secs(3);
    let children = loop {
        let state = f.ok(&["audio", "status"]);
        if state["last"]["token"] != "owned-test" && state["last"]["children"].is_object() {
            break handles(&state);
        }
        assert!(
            Instant::now() < limit,
            "Did not observe failure fixture children before exit"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(!failing_start.wait().unwrap().success());
    assert_exited(children);
    let stopped = f.ok(&["audio", "status"]);
    assert_eq!(stopped["running"], false);
    assert_eq!(stopped["last"]["children_exited"], true);
    assert_eq!(stopped["last"]["restoration"], "verified");
}
