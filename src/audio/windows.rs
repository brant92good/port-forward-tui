use super::*;
use crate::process::{self, AudioGroup};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::{Child, Command, Stdio},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, ReleaseMutex, ResetEvent,
        SetEvent, WaitForSingleObject,
    },
};

fn name(machine: &Machine, kind: &str) -> Vec<u16> {
    let path = machine
        .directory
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    format!(
        "Local\\PortsAudio-{:x}-{kind}\0",
        Sha256::digest(path.as_bytes())
    )
    .encode_utf16()
    .collect()
}
fn mutex(machine: &Machine) -> Result<OwnedHandle> {
    let raw = unsafe { CreateMutexW(ptr::null(), 0, name(machine, "owner").as_ptr()) };
    ensure!(
        !raw.is_null(),
        "Cannot create audio ownership mutex: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}
fn acquire(handle: &OwnedHandle) -> Result<bool> {
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => anyhow::bail!(
            "Cannot inspect audio ownership: {}",
            std::io::Error::last_os_error()
        ),
    }
}
pub fn running(machine: &Machine) -> Result<bool> {
    let handle = mutex(machine)?;
    if acquire(&handle)? {
        unsafe {
            ReleaseMutex(handle.as_raw_handle());
        }
        Ok(false)
    } else {
        Ok(true)
    }
}
pub(super) struct Owner(OwnedHandle);
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.as_raw_handle());
        }
    }
}

pub(super) fn command_guard(machine: &Machine) -> Result<Owner> {
    let raw = unsafe { CreateMutexW(ptr::null(), 0, name(machine, "commands").as_ptr()) };
    ensure!(!raw.is_null(), "Cannot create audio command mutex");
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 30000) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Owner(handle)),
        _ => anyhow::bail!("Another audio command is still finishing; retry shortly"),
    }
}

/// Spawn suspended, attach a kill-on-close job, then resume. Abrupt worker death
/// closes every job handle, including ProxyCommand descendants, before any restart.
struct OwnedChild {
    child: Child,
    group: AudioGroup,
}
impl OwnedChild {
    fn spawn(command: &mut Command) -> Result<Self> {
        process::configure_audio_child(command);
        let mut child = command.spawn()?;
        match AudioGroup::attach(&mut child) {
            Ok(group) => Ok(Self { child, group }),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
    fn stop(&mut self) {
        self.group.stop(&mut self.child);
    }
    fn exited(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ssh(machine: &Machine, config: &Config, action: &str) -> Result<Command> {
    let mut command = Command::new(process::ssh_executable()?);
    command.args(ssh_args(machine, config, action)?);
    Ok(command)
}
fn remote(machine: &Machine, config: &Config, action: &str) -> Result<Value> {
    // File-backed diagnostic/control output only. PCM never enters these files.
    let temp = tempfile::tempdir()?;
    let output_path = temp.path().join("status.json");
    let error_path = temp.path().join("error.txt");
    let mut command = ssh(machine, config, action)?;
    command
        .stdin(Stdio::null())
        .stdout(File::create(&output_path)?)
        .stderr(File::create(&error_path)?);
    let mut child = OwnedChild::spawn(&mut command)?;
    let deadline = Instant::now() + Duration::from_secs(14);
    while !child.exited()? {
        ensure!(
            Instant::now() < deadline,
            "Remote audio {action} timed out; restoration is unconfirmed"
        );
        ensure!(
            fs::metadata(&output_path)?.len() <= 65536 && fs::metadata(&error_path)?.len() <= 65536,
            "Remote audio control output exceeded its limit"
        );
        thread::sleep(Duration::from_millis(50));
    }
    ensure!(
        child.child.try_wait()?.is_some_and(|s| s.success()),
        "Remote audio {action} failed: {}",
        read_text(&error_path)?
    );
    let value: Value = serde_json::from_str(&read_text(&output_path)?)
        .context("Remote stdout was not bridge status JSON (check login banners)")?;
    ensure!(
        value["host"] == config.expected_host,
        "SSH reached an unexpected host: {}",
        value["host"]
    );
    Ok(value)
}
fn read_text(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "Audio diagnostic exceeded 64 KiB");
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn write_state(machine: &Machine, state: &Value) -> Result<()> {
    store::write_json(&machine.directory.join("audio-state.json"), state)
}

pub fn start(machine: &Machine, root: &Path) -> Result<Value> {
    let _command = command_guard(machine)?;
    let config =
        load(machine)?.context("Audio is disabled. Use the optional audio setup first.")?;
    ensure!(
        config.enabled,
        "Audio is disabled. Configure it explicitly before Start."
    );
    ensure!(
        config.ffmpeg.is_file() && config.ffplay.is_file(),
        "Configured FFmpeg/FFplay is missing"
    );
    if running(machine)? {
        return status(machine);
    }
    let token = uuid::Uuid::new_v4().to_string();
    let args: Vec<OsString> = vec![
        "--data-dir".into(),
        root.into(),
        "--machine".into(),
        machine.id.clone().into(),
        "audio".into(),
        "run".into(),
        "--token".into(),
        token.clone().into(),
    ];
    let log = File::create(machine.directory.join("audio-worker.log"))?;
    let mut worker = process::spawn_audio_worker(&std::env::current_exe()?, &args, log)?;
    let deadline = Instant::now() + Duration::from_secs(22);
    loop {
        let current = status(machine)?;
        if current["running"] == true
            && current["last"]["token"] == token
            && current["last"]["phase"] == "streaming"
        {
            return Ok(current);
        }
        if worker.try_wait()?.is_some() {
            anyhow::bail!(
                "Audio did not start: {}. See {}",
                current["last"],
                machine.directory.join("audio-worker.log").display()
            );
        }
        if Instant::now() >= deadline {
            worker.kill()?;
            let limit = Instant::now() + Duration::from_secs(3);
            while worker.try_wait()?.is_none() {
                ensure!(
                    Instant::now() < limit,
                    "Worker termination is unconfirmed; inspect audio status"
                );
                thread::sleep(Duration::from_millis(25));
            }
            anyhow::bail!(
                "Audio startup timed out. Owned worker terminated; remote restoration is unconfirmed. Inspect audio status before retrying."
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn signal_stop(machine: &Machine) -> Result<()> {
    let raw = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name(machine, "stop").as_ptr()) };
    ensure!(
        !raw.is_null(),
        "Audio worker is starting; retry Stop shortly"
    );
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    ensure!(
        unsafe { SetEvent(event.as_raw_handle()) } != 0,
        "Cannot signal audio Stop"
    );
    Ok(())
}
pub fn stop(machine: &Machine) -> Result<Value> {
    let _command = command_guard(machine)?;
    if !running(machine)? {
        return status(machine);
    }
    signal_stop(machine)?;
    let deadline = Instant::now() + Duration::from_secs(38);
    while running(machine)? {
        ensure!(
            Instant::now() < deadline,
            "Audio Stop has not finished; inspect audio status/log (no unrelated process was killed)"
        );
        thread::sleep(Duration::from_millis(100));
    }
    status(machine)
}

pub fn run(machine: &Machine, token: &str) -> Result<()> {
    let handle = mutex(machine)?;
    ensure!(acquire(&handle)?, "Audio already running for this machine");
    let _owner = Owner(handle);
    let raw = unsafe { CreateEventW(ptr::null(), 1, 0, name(machine, "stop").as_ptr()) };
    ensure!(!raw.is_null(), "Cannot create audio Stop event");
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    // Only the current mutex owner may reset the event before admitting capture.
    ensure!(
        unsafe { ResetEvent(event.as_raw_handle()) } != 0,
        "Cannot reset audio Stop event"
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))?;
    let stop_requested = || {
        cancelled.load(Ordering::Relaxed)
            || unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } == WAIT_OBJECT_0
    };
    let mut state = json!({"phase":"starting","token":token,"worker_pid":std::process::id(),"restoration":"not needed: transport not started"});
    write_state(machine, &state)?;
    let mut capture: Option<OwnedChild> = None;
    let mut transport: Option<OwnedChild> = None;
    let mut playback: Option<OwnedChild> = None;
    let mut before = None;
    let mut transport_started = false;
    let mut config_snapshot = None;
    let result = (|| -> Result<()> {
        let config = load(machine)?.context("Audio is not configured")?;
        ensure!(config.enabled, "Audio is disabled");
        let initial = remote(machine, &config, "status")?;
        ensure!(
            initial["state"]["active"] == false,
            "Remote audio is already active; stop the existing bridge first"
        );
        before = Some(initial["defaults"].clone());
        config_snapshot = Some(config.clone());
        if stop_requested() {
            return Ok(());
        }
        let capture_log = machine.directory.join("audio-capture.log");
        let transport_log = machine.directory.join("audio-ssh.log");
        let playback_log = machine.directory.join("audio-playback.log");
        let mut command = Command::new(&config.ffmpeg);
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "dshow",
                "-audio_buffer_size",
                "50",
                "-i",
            ])
            .arg(format!("audio={}", config.microphone))
            .args([
                "-vn",
                "-ac",
                "1",
                "-ar",
                "48000",
                "-acodec",
                "pcm_s16le",
                "-f",
                "s16le",
                "-flush_packets",
                "1",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(&capture_log)?);
        capture = Some(OwnedChild::spawn(&mut command)?);
        drop(command);
        let mut command = ssh(machine, &config, "duplex")?;
        command
            .stdin(Stdio::from(
                capture
                    .as_mut()
                    .unwrap()
                    .child
                    .stdout
                    .take()
                    .context("No microphone pipe")?,
            ))
            .stdout(Stdio::piped())
            .stderr(File::create(&transport_log)?);
        transport = Some(OwnedChild::spawn(&mut command)?);
        transport_started = true;
        drop(command);
        let mut command = Command::new(&config.ffplay);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nodisp",
                "-autoexit",
                "-f",
                "s16le",
                "-sample_rate",
                "48000",
                "-ch_layout",
                "mono",
                "-probesize",
                "32",
                "-analyzeduration",
                "0",
                "-i",
                "pipe:0",
            ])
            .stdin(Stdio::from(
                transport
                    .as_mut()
                    .unwrap()
                    .child
                    .stdout
                    .take()
                    .context("No reply pipe")?,
            ))
            .stdout(Stdio::null())
            .stderr(File::create(&playback_log)?);
        playback = Some(OwnedChild::spawn(&mut command)?);
        drop(command);
        state["children"] = json!({"capture":capture.as_ref().unwrap().child.id(),"ssh":transport.as_ref().unwrap().child.id(),"playback":playback.as_ref().unwrap().child.id()});
        state["before_defaults"] = before.clone().unwrap();
        state["restoration"] = json!("pending until Stop");
        write_state(machine, &state)?;
        let began = Instant::now();
        while !stop_requested() {
            for (label, child, log) in [
                ("microphone", &mut capture, &capture_log),
                ("SSH", &mut transport, &transport_log),
                ("playback", &mut playback, &playback_log),
            ] {
                ensure!(
                    !child.as_mut().unwrap().exited()?,
                    "{label} exited: {}",
                    read_text(log).unwrap_or_default()
                );
                ensure!(
                    fs::metadata(log)?.len() <= 65536,
                    "{label} diagnostic limit reached; stopping audio"
                );
            }
            if state["phase"] == "starting" {
                if began.elapsed() >= Duration::from_secs(1)
                    && read_text(&transport_log)?.contains("SSH voice ready:")
                {
                    state["phase"] = json!("streaming");
                    state["notice"] = json!(
                        "Transport ready; physical microphone/headphones and /voice still require user verification"
                    );
                    write_state(machine, &state)?;
                } else {
                    ensure!(
                        began.elapsed() < Duration::from_secs(15),
                        "Remote audio readiness was not received"
                    );
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })();
    // Stop physical capture first. Dropping its only writer sends EOF through
    // SSH stdin, giving the remote duplex finally block time to restore defaults.
    drop(capture.take());
    state["phase"] = json!("stopping");
    let _ = write_state(machine, &state);
    if let Some(child) = transport.as_mut() {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !child.exited().unwrap_or(true) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }
    drop(transport.take());
    drop(playback.take());
    if let (true, Some(config), Some(defaults)) = (transport_started, &config_snapshot, &before) {
        // deactivate is serialized by the server's nonblocking duplex lock;
        // it refuses to modify any concurrently active duplex session.
        let restored = remote(machine, config, "status").and_then(|status| {
            if status["state"]["active"] == false {
                Ok(status)
            } else {
                remote(machine, config, "deactivate")
            }
        });
        match restored {
            Ok(after) => {
                state["after_defaults"] = after["defaults"].clone();
                state["restoration"] = json!(if after["state"]["active"] == false
                    && after["defaults"] == *defaults
                {
                    "verified"
                } else {
                    "defaults changed elsewhere; inspect remote status"
                });
            }
            Err(error) => state["restoration"] = json!(format!("unconfirmed: {error:#}")),
        }
    }
    state["phase"] = json!(if result.is_ok() { "stopped" } else { "failed" });
    if let Err(error) = &result {
        state["error"] = json!(format!("{error:#}"));
    }
    state["children_exited"] = json!(true);
    write_state(machine, &state)?;
    result
}
