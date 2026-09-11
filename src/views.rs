//! Windows Terminal view identity. All desktop work stays in the existing,
//! precompiled accessibility helpers; failures never guess from foreground.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub fn read_scope(directory: &Path) -> Result<String> {
    let path = directory.join("ui-settings.json");
    if !path.exists() {
        return Ok("all".into());
    }
    let data: Value = serde_json::from_slice(&fs::read(path)?)?;
    data.as_object()
        .context("ui-settings.json must be an object")?;
    let value = match data.get("focus_scope") {
        None => "all",
        Some(value) => value
            .as_str()
            .context("focus_scope must be 'all' or 'window'")?,
    };
    match value {
        "all" => Ok("all".into()),
        "window" => Ok("window".into()),
        _ => bail!("focus_scope must be 'all' or 'window' in ui-settings.json"),
    }
}

pub fn save_scope(directory: &Path, scope: &str) -> Result<()> {
    if !matches!(scope, "all" | "window") {
        bail!("Unknown focus scope");
    }
    fs::create_dir_all(directory)?;
    let path = directory.join("ui-settings.json");
    let mut data: Value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?)?
    } else {
        json!({})
    };
    data.as_object_mut()
        .context("ui-settings.json must be an object")?
        .insert("focus_scope".into(), json!(scope));
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(serde_json::to_string_pretty(&data)?.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn helper(name: &str) -> Result<PathBuf> {
    let path = std::env::current_exe()?
        .parent()
        .context("Executable has no directory")?
        .join(name);
    if !path.is_file() {
        bail!("Missing {}. Reinstall the Windows binary bundle.", name);
    }
    Ok(path)
}

#[cfg(windows)]
fn hidden(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}
#[cfg(not(windows))]
fn hidden(_: &mut Command) {}

fn bounded(command: &mut Command, timeout: Duration) -> Result<std::process::Output> {
    use std::io::Read;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hidden(command);
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().context("Missing helper output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .by_ref()
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Terminal helper timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Helper output failed"))??;
    if stdout.len() > 1_048_576 {
        bail!("Terminal helper returned too much output");
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr: vec![],
    })
}

pub fn mark_origin() -> Result<String> {
    let title = format!("Shortcut | {}", uuid::Uuid::new_v4().simple());
    set_title(&title)?;
    Ok(title)
}

#[cfg(windows)]
fn set_title(title: &str) -> Result<()> {
    // stdout may be the parent's selection-result pipe. The inherited console
    // input, not stdout, establishes that this is an interactive invocation.
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleTitleW,
    };
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let mut mode = 0;
    unsafe {
        if GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) == 0
            || SetConsoleTitleW(title.as_ptr()) == 0
        {
            bail!("Cannot identify the current Terminal tab");
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn set_title(_: &str) -> Result<()> {
    bail!("Window return shortcuts require Windows Terminal")
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == 258;
        CloseHandle(handle);
        alive
    }
}
#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
fn process_started(pid: u32) -> Option<u64> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut times = [FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        }; 4];
        let success = GetProcessTimes(
            handle,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        );
        CloseHandle(handle);
        (success != 0).then(|| {
            ((times[0].dwHighDateTime as u64) << 32 | times[0].dwLowDateTime as u64)
                + 504_911_232_000_000_000
        })
    }
}
#[cfg(not(windows))]
fn process_started(_: u32) -> Option<u64> {
    None
}

fn live_records(directory: &Path) -> Vec<Value> {
    let mut records = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        if bytes.len() > 65_536 {
            continue;
        }
        let Ok(record) =
            serde_json::from_slice::<Value>(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
        else {
            continue;
        };
        let Some(pid) = record["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
            continue;
        };
        if !process_alive(pid) {
            continue;
        }
        if let Some(started) = record["started"].as_u64()
            && process_started(pid) != Some(started)
        {
            continue;
        }
        records.push(record);
    }
    records.sort_by(|a, b| timestamp(b).total_cmp(&timestamp(a)));
    records
}

fn timestamp(record: &Value) -> f64 {
    let time = record["last_focus"].as_f64().unwrap_or(0.0);
    if time > 1e14 {
        (time - 621_355_968_000_000_000.0) / 10_000_000.0
    } else {
        time
    }
}

pub fn machine_for_invocation(root: &Path) -> Result<Option<String>> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let origin = mark_origin()?;
    let output = bounded(
        Command::new(helper("PortsFocus.exe")?).args(["-ResolveOrigin", "-OriginTitle", &origin]),
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let data: Value = serde_json::from_slice(&output.stdout)?;
    let Some(window) = data["window"].as_u64().filter(|v| *v != 0) else {
        return Ok(None);
    };
    Ok(live_records(&root.join("window-views"))
        .into_iter()
        .find_map(|record| {
            if record["window"].as_u64() == Some(window) && record["started"].as_u64().is_some() {
                record["machine"].as_str().map(str::to_owned)
            } else {
                None
            }
        }))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for part in bytes.chunks(3) {
        let n = (part[0] as u32) << 16
            | (part.get(1).copied().unwrap_or(0) as u32) << 8
            | part.get(2).copied().unwrap_or(0) as u32;
        text.push(TABLE[(n >> 18) as usize] as char);
        text.push(TABLE[((n >> 12) & 63) as usize] as char);
        text.push(if part.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        text.push(if part.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    text
}

pub fn try_focus(records_dir: &Path, scope: &str, origin: &str, probe: bool) -> Result<bool> {
    if !cfg!(windows) {
        return Ok(false);
    }
    if !matches!(scope, "all" | "window") {
        bail!("Unknown focus scope");
    }
    if origin.is_empty() && (!probe || scope == "window") {
        return Ok(false);
    }
    let mut records = live_records(records_dir);
    if records.is_empty() {
        return Ok(false);
    }
    // The helper compares .NET ticks. Legacy Ports used Unix seconds, so
    // normalize before one MRU lookup across old and new native records.
    for record in &mut records {
        record["last_focus"] =
            json!((timestamp(record) * 10_000_000.0 + 621_355_968_000_000_000.0) as u64);
        if record["started"].is_null() {
            // Older records without an owner start time cannot prove identity.
            record["started"] = json!(0);
        }
    }
    let data = serde_json::to_vec(&records)?;
    let mut command = Command::new(helper("PortsFocus.exe")?);
    command.args(["-RecordsBase64", &base64(&data), "-Scope", scope]);
    if !origin.is_empty() {
        command.args(["-OriginTitle", origin]);
    }
    if probe {
        command.arg("-ProbeOnly");
        return Ok(bounded(&mut command, Duration::from_secs(5))?
            .status
            .success());
    }
    native_focus(command)
}

#[cfg(windows)]
fn native_focus(mut command: Command) -> Result<bool> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CreateEventW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForMultipleObjects,
        },
        UI::WindowsAndMessaging::AllowSetForegroundWindow,
    };
    let name = format!("Local\\PortsFocus-{}", uuid::Uuid::new_v4().simple());
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let event = CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr());
        if event.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        command.args([
            "-ReadyEvent",
            &name,
            "-AfterPid",
            &std::process::id().to_string(),
        ]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000 | 0x01000000);
        let result = (|| -> Result<bool> {
            let mut child = command.spawn()?;
            AllowSetForegroundWindow(child.id());
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, child.id());
            if process.is_null() {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            let handles = [event, process];
            let matched = WaitForMultipleObjects(2, handles.as_ptr(), 0, 5000) == 0;
            CloseHandle(process);
            if !matched {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(matched)
        })();
        CloseHandle(event);
        result
    }
}
#[cfg(not(windows))]
fn native_focus(_: Command) -> Result<bool> {
    Ok(false)
}

pub struct ViewRegistration {
    tracker: Option<Child>,
    record: PathBuf,
    context: Option<PathBuf>,
    pub title: String,
    ports_data: Option<Value>,
}
impl Drop for ViewRegistration {
    fn drop(&mut self) {
        if let Some(tracker) = &mut self.tracker {
            let _ = tracker.kill();
            let _ = tracker.wait();
        }
        let _ = fs::remove_file(&self.record);
        if let Some(path) = &self.context {
            let _ = fs::remove_file(path);
        }
    }
}

impl ViewRegistration {
    fn write_ports(&self) -> Result<()> {
        use std::io::Write;
        let Some(data) = &self.ports_data else {
            return Ok(());
        };
        for path in std::iter::once(&self.record).chain(self.context.iter()) {
            let parent = path.parent().context("View record has no directory")?;
            fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&serde_json::to_vec(data)?)?;
            temporary.persist(path)?;
        }
        Ok(())
    }
    pub fn focused(&mut self) -> Result<()> {
        if let Some(data) = self.ports_data.as_mut() {
            #[cfg(windows)]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                let window = unsafe { GetForegroundWindow() } as usize as u64;
                if window == 0 || data["window"].as_u64() != Some(window) {
                    return Ok(());
                }
            }
            data["last_focus"] = json!(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs_f64()
            );
            self.write_ports()?;
        }
        Ok(())
    }
    pub fn select_machine(
        &mut self,
        directory: &Path,
        catalog_root: &Path,
        machine: &str,
        target: &str,
    ) -> Result<()> {
        let data = self
            .ports_data
            .as_mut()
            .context("Only Ports views can change machines")?;
        let name = self
            .record
            .file_name()
            .context("View record has no name")?
            .to_owned();
        let stem = self.record.file_stem().unwrap().to_string_lossy();
        self.title = format!("Ports BETA | {} | {}", target, &stem[..6]);
        data["title"] = json!(self.title);
        data["machine"] = json!(machine);
        let previous = self.record.clone();
        self.record = directory.join("views").join(&name);
        self.context = Some(catalog_root.join("window-views").join(name));
        set_title(&self.title)?;
        self.write_ports()?;
        if previous != self.record {
            let _ = fs::remove_file(previous);
        }
        self.focused()
    }
}

pub fn register_remote(
    records_dir: &Path,
    catalog_root: Option<&Path>,
    machine: Option<&str>,
    origin: &str,
    tracker: &Path,
) -> Result<ViewRegistration> {
    if origin.is_empty() {
        bail!("A unique tab origin is required");
    }
    if !tracker.is_file() {
        bail!("Missing TerminalViews.exe. Reinstall the Windows binary bundle.");
    }
    fs::create_dir_all(records_dir)?;
    let name = format!("{}.json", uuid::Uuid::new_v4().simple());
    let record = records_dir.join(&name);
    let context = catalog_root
        .zip(machine)
        .map(|(root, _)| root.join("window-views").join(&name));
    if let Some(path) = &context {
        fs::create_dir_all(path.parent().unwrap())?;
    }
    let mut command = Command::new(tracker);
    command
        .args(["-Mode", "Track", "-RecordPath"])
        .arg(&record)
        .args([
            "-InitialTitle",
            origin,
            "-OwnerPid",
            &std::process::id().to_string(),
        ]);
    if let (Some(machine), Some(context)) = (machine, context.as_ref()) {
        command
            .args(["-MachineId", machine, "-ContextPath"])
            .arg(context);
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(records_dir.join("tracker.log"))?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    hidden(&mut command);
    let mut view = ViewRegistration {
        tracker: Some(command.spawn()?),
        record,
        context,
        title: origin.into(),
        ports_data: None,
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    while !view.record.is_file()
        && view.tracker.as_mut().unwrap().try_wait()?.is_none()
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(25));
    }
    if !view.record.is_file() {
        bail!("This tab could not register for the return shortcut");
    }
    Ok(view)
}

pub fn register_ports(
    directory: &Path,
    catalog_root: &Path,
    machine: &str,
    target: &str,
) -> Result<ViewRegistration> {
    let title = format!(
        "Ports BETA | {} | {}",
        target,
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    set_title(&title)?;
    let mut view = register_remote(
        &directory.join("views"),
        Some(catalog_root),
        Some(machine),
        &title,
        &helper("TerminalViews.exe")?,
    )?;
    // Capture the verified identity once, then handle focus/input events locally.
    // Machine movement in a combined list never starts another helper.
    let mut data: Value = serde_json::from_slice(&fs::read(&view.record)?)?;
    if let Some(mut tracker) = view.tracker.take() {
        let _ = tracker.kill();
        let _ = tracker.wait();
    }
    data["title"] = json!(title);
    view.ports_data = Some(data);
    view.write_ports()?;
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_encoding() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"foo"), "Zm9v");
    }
    #[test]
    fn mixed_timestamps_sort_consistently() {
        assert_eq!(
            timestamp(&json!({"last_focus": 621_355_968_010_000_000u64})),
            1.0
        );
    }
    #[test]
    fn scope_preserves_other_preferences() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ui-settings.json"), "{\"theme\":\"teal\"}").unwrap();
        save_scope(dir.path(), "window").unwrap();
        assert_eq!(read_scope(dir.path()).unwrap(), "window");
        assert!(
            fs::read_to_string(dir.path().join("ui-settings.json"))
                .unwrap()
                .contains("teal")
        );
        assert!(save_scope(dir.path(), "guess").is_err());
    }
}
