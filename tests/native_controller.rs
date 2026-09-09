use port_forward_tui::{
    background,
    machines::Catalog,
    store::{Forward, Lock, Store},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    net::{Ipv4Addr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}
fn cli(root: &Path, args: &[&str]) -> (i32, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ports"));
    command
        .args(["--data-dir", root.to_str().unwrap(), "--json"])
        .args(args)
        .stdin(Stdio::null());
    hidden(&mut command);
    let output = command.output().unwrap();
    let parsed = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "Invalid JSON: {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap(), parsed)
}
struct Controller {
    child: Child,
    path: PathBuf,
}
impl Controller {
    fn start(path: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ports"));
        command
            .arg("--serve")
            .arg("--data-dir")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hidden(&mut command);
        let child = command.spawn().unwrap();
        let result = Self {
            child,
            path: path.into(),
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if background::exchange(path, "status", json!({}), Duration::from_millis(200)).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "Controller did not start");
            thread::sleep(Duration::from_millis(25));
        }
        result
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        let _ = background::exchange(&self.path, "shutdown", json!({}), Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(4);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn real_cli_readonly_and_picker_contract() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("never created");
    for args in [&["list"][..], &["machines", "list"], &["doctor"]] {
        let (code, result) = cli(&absent, args);
        assert_eq!(code, 0, "{result}");
        assert_eq!(result["schema_version"], 1);
        assert!(!absent.exists());
    }
    let (code, result) = cli(&absent, &["start", "bad", "--wait", "nan"]);
    assert_eq!(code, 2);
    assert_eq!(result["error"]["code"], "invalid_arguments");
    let (code, result) = cli(&absent, &["delete", "bad"]);
    assert_eq!(code, 2);
    assert_eq!(result["ok"], false);
    let catalog = Catalog::new(&absent).unwrap();
    let one = catalog.add("workbox", "Dev box", None, None).unwrap();
    let two = catalog.add("other", "Other box", Some(2222), None).unwrap();
    let (code, result) = cli(
        &absent,
        &[
            "machines",
            "pick",
            "--machine",
            &one.id,
            "--no-window-context",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(result["command"], "machines pick");
    assert_eq!(
        result["machine"]["directory"],
        one.directory.to_str().unwrap()
    );
    assert_eq!(result["machine"]["ssh_port"], Value::Null);
    let (code, result) = cli(&absent, &["list"]);
    assert_eq!(code, 0);
    assert_eq!(result["machines"].as_array().unwrap().len(), 2);
    assert!(
        result["forwards"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "UNKNOWN")
    );
    assert!(!one.directory.join("endpoint.json").exists());
    assert!(!two.directory.join("endpoint.json").exists());
}
#[test]
fn real_ipc_concurrent_edits_slow_clients_auth_and_broken_file_stop() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = catalog.add("workbox", "Test", None, None).unwrap();
    let controller = Controller::start(&machine.directory);
    assert!(Lock::acquire(&machine.directory, "daemon.lock", Duration::ZERO).is_err());
    let snapshot = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(snapshot["protocol"], 1);
    assert!(snapshot["states"].as_object().unwrap().is_empty());
    let rule = Forward::new(19287, 9287, "One").unwrap();
    background::exchange(
        &machine.directory,
        "upsert",
        json!({"rule":rule}),
        Duration::from_secs(2),
    )
    .unwrap();
    let mut edited = rule.clone();
    edited.name = "Changed from second view".into();
    background::exchange(
        &machine.directory,
        "upsert",
        json!({"rule":edited,"expected":rule}),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(
        background::exchange(
            &machine.directory,
            "upsert",
            json!({"rule":rule,"expected":rule}),
            Duration::from_secs(2)
        )
        .is_err()
    );
    assert!(
        background::exchange(
            &machine.directory,
            "delete",
            json!({"rule_id":rule.id,"expected":rule}),
            Duration::from_secs(2)
        )
        .is_err()
    );
    let endpoint: background::Endpoint =
        serde_json::from_slice(&fs::read(machine.directory.join("endpoint.json")).unwrap())
            .unwrap();
    let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
    slow.write_all(b"{").unwrap();
    let start = Instant::now();
    assert!(
        background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_secs(1)
        )
        .is_ok()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    fs::write(machine.directory.join("forwards.json"), b"broken").unwrap();
    assert!(
        background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_secs(2)
        )
        .is_err()
    );
    assert!(background::call(&machine.directory, "stop_all", json!({})).is_ok());
    drop(slow);
    drop(controller);
    assert!(!machine.directory.join("endpoint.json").exists());
}
#[test]
fn string_ports_roundtrip_without_creating_or_replacing_favorites() {
    let temp = tempfile::tempdir().unwrap();
    let payload = json!({"version":1,"host":"workbox","ssh_port":"2222","forwards":[{"id":"0123456789abcdef0123456789abcdef","name":"API","local_port":"18000","remote_port":"8000"}]});
    fs::write(
        temp.path().join("forwards.json"),
        serde_json::to_vec(&payload).unwrap(),
    )
    .unwrap();
    let mut store = Store::load(temp.path()).unwrap();
    assert!(store.settings.keep_alive);
    assert_eq!(store.settings.ssh_port, Some(2222));
    assert_eq!(store.settings.forwards.len(), 1);
    store.save(store.settings.forwards.clone()).unwrap();
    assert_eq!(Store::load(temp.path()).unwrap().settings.forwards.len(), 1);
    for value in [json!(true), json!(0), json!(1.5), json!(65536)] {
        let mut bad = payload.clone();
        bad["ssh_port"] = value;
        fs::write(
            temp.path().join("forwards.json"),
            serde_json::to_vec(&bad).unwrap(),
        )
        .unwrap();
        assert!(Store::load(temp.path()).is_err());
    }
}

#[test]
fn controller_waits_for_a_connected_client_to_send_its_request() {
    use std::io::{BufRead, BufReader};
    let temp = tempfile::tempdir().unwrap();
    let machine = Catalog::new(temp.path())
        .unwrap()
        .add("workbox", "Delayed client", None, None)
        .unwrap();
    let _controller = Controller::start(&machine.directory);
    let endpoint: background::Endpoint =
        serde_json::from_slice(&fs::read(machine.directory.join("endpoint.json")).unwrap())
            .unwrap();
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    // A client may be descheduled after connect, or send a request in pieces.
    // Windows accepted sockets inherit the listener's nonblocking mode.
    thread::sleep(Duration::from_millis(100));
    let request = json!({"protocol": 1, "token": endpoint.token, "command": "status"});
    let bytes = serde_json::to_vec(&request).unwrap();
    stream.write_all(&bytes[..8]).unwrap();
    thread::sleep(Duration::from_millis(100));
    stream.write_all(&bytes[8..]).unwrap();
    stream.write_all(b"\n").unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["pid"], endpoint.pid);
}
