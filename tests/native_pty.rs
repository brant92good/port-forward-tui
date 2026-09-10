//! Real ConPTY/PTY keyboard tests. No desktop windows or personal data.
#[path = "support/controlled_controller.rs"]
mod controlled_controller;
use port_forward_tui::{
    auto_open, background,
    machines::Catalog,
    store::{Forward, Store},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pair: portable_pty::PtyPair,
    input: Box<dyn Write + Send>,
    output: mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
    screen: vt100::Parser,
}
impl Session {
    fn start(root: &Path, machine: &str) -> Self {
        Self::with_mode(root, machine, false)
    }
    fn with_mode(root: &Path, machine: &str, foreground: bool) -> Self {
        Self::with_ssh(root, machine, foreground, None)
    }
    fn with_ssh(root: &Path, machine: &str, foreground: bool, ssh: Option<&Path>) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 32,
                cols: 130,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_ports"));
        command.args(["--data-dir", root.to_str().unwrap(), "--machine", machine]);
        if foreground {
            command.arg("--foreground");
        }
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env_remove("WT_SESSION");
        if let Some(ssh) = ssh {
            command.env("PATH", ssh.parent().unwrap());
        }
        let child = pair.slave.spawn_command(command).unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let input = pair.master.take_writer().unwrap();
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            let mut chunk = [0; 8192];
            while let Ok(count) = reader.read(&mut chunk) {
                if count == 0 || sender.send(chunk[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            _pair: pair,
            input,
            output,
            seen: Vec::new(),
            screen: vt100::Parser::new(32, 130, 0),
        }
    }
    fn pump(&mut self) {
        if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(50)) {
            self.screen.process(&bytes);
            self.seen.extend(bytes);
        }
        if let Some(index) = self.seen.windows(4).position(|part| part == b"\x1b[6n") {
            self.seen.drain(index..index + 4);
            self.send("\x1b[1;1R");
        }
    }
    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            if self.screen.screen().contents().contains(text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Missing {text:?}: {}",
                self.screen.screen().contents()
            );
        }
    }
    fn send(&mut self, text: &str) {
        self.input.write_all(text.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }
    fn expect_row(&mut self, name: &str, state: &str) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            let content = self.screen.screen().contents();
            let lines: Vec<_> = content.lines().collect();
            let named = |line: &str| {
                line.split_once('·')
                    .is_some_and(|(heading, _)| heading.trim_matches(['│', ' ']) == name)
            };
            let has_state = |line: &str| line.split_whitespace().any(|word| word == state);
            // Server headings are separate from Name-first forwarding rows.
            // Small views may have room for only the selected row; in that case
            // require its machine in the top context header, never arbitrary text.
            let selected_context = lines.iter().any(|line| {
                named(line) && (line.contains("Background on") || line.contains("Foreground ·"))
            }) && lines
                .iter()
                .any(|line| line.contains('›') && has_state(line));
            let grouped_context = lines.windows(2).any(|pair| {
                named(pair[0])
                    && !pair[0].contains("Background on")
                    && !pair[0].contains("Foreground ·")
                    && has_state(pair[1])
            });
            if selected_context || grouped_context {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Missing {name} {state} row: {}",
                self.screen.screen().contents()
            );
        }
    }
    fn wait_for(&mut self, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while !condition() {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "Keyboard action did not reach persisted state: {}",
                String::from_utf8_lossy(&self.seen)
            );
        }
    }
    fn close(&mut self) {
        self.send("q");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status:?}");
                return;
            }
            self.pump();
            assert!(Instant::now() < deadline, "View did not close");
        }
    }
}

#[test]
fn foreground_quick_entry_resolves_saved_mapping_before_next_screen_poll() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let machine = catalog
        .add(
            "127.0.0.1",
            "Foreground server",
            Some(listener.local_addr().unwrap().port()),
            None,
        )
        .unwrap();
    drop(listener);
    let mut view = Session::with_mode(temp.path(), &machine.id, true);
    view.expect("Saved connections");
    view.send("n18333:8333 Original API\r");
    view.wait_for(|| {
        Store::load(&machine.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|r| r.name == "Original API")
    });
    let saved = Store::load(&machine.directory)
        .unwrap()
        .settings
        .forwards
        .into_iter()
        .find(|r| r.name == "Original API")
        .unwrap();
    view.send("n18333:8333 Renamed immediately\r");
    view.wait_for(|| {
        Store::load(&machine.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|r| r.id == saved.id && r.name == "Renamed immediately")
    });
    view.close();
    assert!(!machine.directory.join("endpoint.json").exists());
}
impl Drop for Session {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !matches!(self.child.try_wait(), Ok(Some(_))) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
}
struct Cleanup {
    path: std::path::PathBuf,
    child: std::process::Child,
}
impl Cleanup {
    fn start(path: &Path) -> Self {
        Self::with_ssh(path, None)
    }
    fn with_ssh(path: &Path, ssh: Option<&Path>) -> Self {
        // portable-pty deliberately owns its child in a non-breakaway Windows
        // job. Start the shared controller outside that harness job; this test
        // qualifies keyboard/multiple views, not desktop launcher detachment.
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ports"));
        if let Some(ssh) = ssh {
            command.env("PATH", ssh.parent().unwrap());
        }
        command
            .arg("--serve")
            .arg("--data-dir")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command.spawn().unwrap();
        let result = Self {
            path: path.into(),
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while background::exchange(path, "status", json!({}), Duration::from_millis(200)).is_err() {
            assert!(Instant::now() < deadline, "Controller did not start");
            thread::sleep(Duration::from_millis(25));
        }
        result
    }
    #[cfg(windows)]
    fn legacy(path: &Path, python: &Path, ssh: &Path) -> Self {
        use std::os::windows::process::CommandExt;
        let child = std::process::Command::new(python)
            .args(["-E", "-s", "-c", "import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); from port_forward_tui import forwarding,background; forwarding.SSH=sys.argv[2]; background.serve(Path(sys.argv[3]))"])
            .arg(env!("CARGO_MANIFEST_DIR"))
            .arg(ssh)
            .arg(path)
            .env("PATH", ssh.parent().unwrap())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let result = Self {
            path: path.into(),
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while background::exchange(path, "status", json!({}), Duration::from_millis(200)).is_err() {
            assert!(
                Instant::now() < deadline,
                "Legacy fixture controller did not start"
            );
            thread::sleep(Duration::from_millis(25));
        }
        result
    }
}
impl Drop for Cleanup {
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
fn keyboard_form_multihost_concurrent_view_and_detach() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let first_port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let second_port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    catalog
        .add(
            "127.0.0.1",
            "First server",
            Some(first_port.local_addr().unwrap().port()),
            None,
        )
        .unwrap();
    catalog
        .add(
            "127.0.0.1",
            "Second server",
            Some(second_port.local_addr().unwrap().port()),
            None,
        )
        .unwrap();
    drop(first_port);
    drop(second_port);
    let machine = catalog.list().unwrap().pop().unwrap();
    let other = catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|m| m.id != machine.id)
        .unwrap();
    let _cleanup = Cleanup::start(&machine.directory);
    let _other_cleanup = Cleanup::start(&other.directory);
    let mut first = Session::start(temp.path(), &machine.id);
    first.expect("Saved connections");
    first.send("a");
    first.expect("App port on server");
    first.send("9009\t18009\tPTY API\r");
    first.wait_for(|| {
        Store::load(&machine.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|r| r.name == "PTY API")
    });
    let saved = Store::load(&machine.directory)
        .unwrap()
        .settings
        .forwards
        .into_iter()
        .find(|r| r.name == "PTY API")
        .unwrap();
    assert_eq!((saved.local_port, saved.remote_port), (18009, 9009));
    first.wait_for(|| {
        background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_secs(2),
        )
        .is_ok_and(|snapshot| snapshot["states"][&saved.id] == "RETRYING")
    });
    first.send("n18009:9009 Renamed API\r");
    first.wait_for(|| {
        Store::load(&machine.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|rule| rule.id == saved.id && rule.name == "Renamed API")
    });
    // Group headings consume rows: select the saved final row before asserting
    // its visible text rather than assuming the entire list fits this viewport.
    first.send("\x1b[F");
    first.expect("Renamed API");
    first.send("\x1b[Hn18009:9010 Collision\r");
    first.expect("already requested");
    first.send("\x1b");
    first.expect("Quick forward");
    first.send("a");
    first.expect("App port on server");
    first.send("9010\t18010\tOther API\r");
    first.wait_for(|| {
        Store::load(&other.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|rule| rule.name == "Other API")
    });
    // Persistence precedes the command response. Wait for the new main-view
    // row, which is polled after that response, before sending the next change.
    first.expect("Saved connections");
    first.expect("Other API");
    first.send("s");
    first.expect("every listed server");
    first.send("y");
    first.wait_for(|| {
        [&machine, &other].iter().all(|machine| {
            background::exchange(
                &machine.directory,
                "status",
                json!({}),
                Duration::from_secs(2),
            )
            .is_ok_and(|snapshot| {
                snapshot["states"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|state| state == "OFF")
            })
        })
    });
    let mut second = Session::start(temp.path(), &machine.id);
    second.expect("Saved connections");
    second.send("\x1b[F");
    second.expect("Renamed API");
    first.close();
    let snapshot = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(
        snapshot["forwards"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == saved.id)
    );
    second.send("\x1b[F");
    second.send("d");
    second.expect("Delete favorite");
    second.send("y");
    second.wait_for(|| {
        !Store::load(&machine.directory)
            .unwrap()
            .settings
            .forwards
            .iter()
            .any(|r| r.id == saved.id)
    });
    second.close();
    assert!(
        background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_secs(2)
        )
        .is_ok()
    );
    assert!(other.directory.join("endpoint.json").exists());
}

fn fixture_ssh(root: &Path) -> std::path::PathBuf {
    let binary = root.join(if cfg!(windows) { "ssh.exe" } else { "ssh" });
    let mut compiler = std::process::Command::new("rustc");
    compiler
        .args(["--edition=2024", "-o"])
        .arg(&binary)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/ssh_fixture.rs"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        compiler.creation_flags(0x08000000);
    }
    let output = compiler.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}
fn fixture_machine(
    catalog: &Catalog,
    name: &str,
    count: usize,
) -> port_forward_tui::machines::Machine {
    let directory = catalog.directory.join(format!("fixture-{name}"));
    std::fs::create_dir_all(&directory).unwrap();
    let config = directory.join("config");
    std::fs::write(
        &config,
        "# Owned SSH-shaped fixture; never reads user SSH settings\n",
    )
    .unwrap();
    let machine = catalog
        .add(&format!("{name}.invalid"), name, None, Some(&config))
        .unwrap();
    let listeners = (0..count)
        .map(|_| std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap())
        .collect::<Vec<_>>();
    let rules = listeners
        .iter()
        .enumerate()
        .map(|(index, listener)| {
            Forward::new(
                listener.local_addr().unwrap().port(),
                8000 + index as u16,
                &format!("{name} {index}"),
            )
            .unwrap()
        })
        .collect();
    Store::load(&machine.directory)
        .unwrap()
        .save(rules)
        .unwrap();
    machine
}
fn state(machine: &port_forward_tui::machines::Machine, id: &str) -> String {
    background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap()["states"][id]
        .as_str()
        .unwrap_or("OFF")
        .into()
}
fn tunnel_pid(machine: &port_forward_tui::machines::Machine) -> String {
    let directory = Path::new(machine.ssh_config.as_ref().unwrap())
        .parent()
        .unwrap();
    std::fs::read_to_string(directory.join("parent.pid")).unwrap()
}

struct FixtureStop(Vec<std::path::PathBuf>);
impl FixtureStop {
    fn new(root: &Path, machines: &[&port_forward_tui::machines::Machine]) -> Self {
        let root = std::fs::canonicalize(root).unwrap();
        Self(
            machines
                .iter()
                .map(|machine| {
                    let directory = std::fs::canonicalize(
                        Path::new(machine.ssh_config.as_ref().unwrap())
                            .parent()
                            .unwrap(),
                    )
                    .unwrap();
                    assert!(directory.starts_with(&root));
                    directory
                })
                .collect(),
        )
    }
    fn pids(&self) -> Vec<u32> {
        self.0
            .iter()
            .flat_map(|directory| {
                std::fs::read_to_string(directory.join("fixture-pids"))
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| line.parse().ok())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}
fn fixture_running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|stat| {
                stat.rsplit_once(')').is_some_and(|(_, fields)| {
                    !matches!(fields.split_whitespace().next(), Some("Z" | "X"))
                })
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        port_forward_tui::views::process_alive(pid)
    }
}
impl Drop for FixtureStop {
    fn drop(&mut self) {
        // Only uniquely owned temp files are written. No PID is ever signaled;
        // the fixture cooperates only with this explicit cleanup marker.
        for directory in &self.0 {
            let _ = std::fs::write(directory.join("fixture-stop"), "test cleanup");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.pids().iter().copied().any(fixture_running) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        if !std::thread::panicking() {
            assert!(
                !self.pids().iter().copied().any(fixture_running),
                "Owned fixture cleanup timed out"
            );
        }
    }
}

#[test]
fn automatic_settings_new_views_two_hosts_keep_running_pids_and_manual_off() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let first = fixture_machine(&catalog, "First", 1);
    let second = fixture_machine(&catalog, "Second", 1);
    let _fixture_stop = FixtureStop::new(temp.path(), &[&first, &second]);
    let one = Store::load(&first.directory).unwrap().settings.forwards[0].clone();
    let two = Store::load(&second.directory).unwrap().settings.forwards[0].clone();
    auto_open::set(&second.directory, &two.id, true).unwrap();
    let _first_controller = Cleanup::with_ssh(&first.directory, Some(&ssh));
    let _second_controller = Cleanup::with_ssh(&second.directory, Some(&ssh));
    let mut setup = Session::start(temp.path(), &first.id);
    setup.expect("Saved connections");
    setup.wait_for(|| state(&second, &two.id) == "ON");
    assert_eq!(state(&first, &one.id), "OFF");
    let second_pid = tunnel_pid(&second);
    setup.send("\x1bOQ"); // F2: real keyboard settings path
    setup.expect("Open automatically");
    setup.send(" \r");
    setup.wait_for(|| {
        auto_open::Preferences::load(&first.directory)
            .unwrap()
            .enabled(&one.id)
    });
    // Background completion messages may replace the save toast immediately.
    // The main-row detail proves the settings modal closed with the saved value.
    setup.expect("Open automatically: on (F2 settings)");
    assert_eq!(
        state(&first, &one.id),
        "OFF",
        "Changing preference must not start now"
    );
    setup.close();
    let mut left = Session::start(temp.path(), &first.id);
    let mut right = Session::start(temp.path(), &second.id);
    left.expect("Saved connections");
    right.expect("Saved connections");
    left.wait_for(|| state(&first, &one.id) == "ON");
    right.expect("already requested");
    let first_pid = tunnel_pid(&first);
    assert_eq!(tunnel_pid(&second), second_pid);
    left.expect_row("First", "ON");
    left.send("\r");
    left.wait_for(|| state(&first, &one.id) == "OFF");
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        left.pump();
        right.pump();
        assert_eq!(
            state(&first, &one.id),
            "OFF",
            "Refresh must not reapply opening intent"
        );
    }
    assert!(
        auto_open::Preferences::load(&first.directory)
            .unwrap()
            .enabled(&one.id)
    );
    assert_eq!(state(&second, &two.id), "ON");
    left.close();
    right.close();
    let mut reopened = Session::start(temp.path(), &first.id);
    reopened.expect("Saved connections");
    reopened.wait_for(|| state(&first, &one.id) == "ON");
    assert_ne!(tunnel_pid(&first), first_pid);
    assert_eq!(tunnel_pid(&second), second_pid);
    reopened.close();
    std::fs::write(first.directory.join(auto_open::FILE), "invalid options").unwrap();
    let mut damaged = Session::start(temp.path(), &first.id);
    damaged.expect("Automatic opening disabled");
    damaged.expect_row("First", "ON");
    // Invalid sidecar cannot take away manual Stop or rewrite itself.
    damaged.send("\r");
    damaged.wait_for(|| state(&first, &one.id) == "OFF");
    assert_eq!(
        std::fs::read_to_string(first.directory.join(auto_open::FILE)).unwrap(),
        "invalid options"
    );
    assert_eq!(state(&second, &two.id), "ON");
    damaged.close();
}

#[test]
fn automatic_queue_quit_and_stop_all_cancel_unsent_work() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = fixture_machine(&catalog, "Queue", 30);
    let _fixture_stop = FixtureStop::new(temp.path(), &[&machine]);
    let rules = Store::load(&machine.directory).unwrap().settings.forwards;
    for rule in &rules {
        auto_open::set(&machine.directory, &rule.id, true).unwrap();
    }
    let _controller = Cleanup::with_ssh(&machine.directory, Some(&ssh));
    let mut first = Session::start(temp.path(), &machine.id);
    first.expect("QUEUED");
    first.close();
    let snapshot = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    let requested = snapshot["states"].as_object().unwrap().len();
    assert!(requested < rules.len(), "Quit must cancel the unsent queue");
    background::exchange(
        &machine.directory,
        "stop_all",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    let mut second = Session::start(temp.path(), &machine.id);
    second.expect("QUEUED");
    second.send("s");
    second.expect("every listed server");
    second.send("y");
    // An old OFF snapshot can arrive before an in-flight automatic start and
    // the queued Stop-all complete. This fresh view has no earlier manual
    // operation, so its Updated notice is the Stop-all completion boundary.
    second.expect("Updated");
    second.wait_for(|| {
        background::exchange(
            &machine.directory,
            "status",
            json!({}),
            Duration::from_secs(2),
        )
        .unwrap()["states"]
            .as_object()
            .unwrap()
            .values()
            .all(|value| value == "OFF")
    });
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        second.pump();
        assert!(rules.iter().all(|rule| state(&machine, &rule.id) == "OFF"));
    }
    assert_eq!(
        auto_open::Preferences::load(&machine.directory)
            .unwrap()
            .open_automatically
            .len(),
        rules.len()
    );
    second.close();
}

#[test]
fn automatic_open_respects_other_machines_retry_reservation() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let first = fixture_machine(&catalog, "First", 1);
    let second = fixture_machine(&catalog, "Second", 1);
    let _fixture_stop = FixtureStop::new(temp.path(), &[&first, &second]);
    let one = Store::load(&first.directory).unwrap().settings.forwards[0].clone();
    let mut two = Store::load(&second.directory).unwrap().settings.forwards[0].clone();
    two.local_port = one.local_port;
    Store::load(&second.directory)
        .unwrap()
        .save(vec![two.clone()])
        .unwrap();
    auto_open::set(&second.directory, &two.id, true).unwrap();
    let _a = Cleanup::with_ssh(&first.directory, Some(&ssh));
    let _b = Cleanup::with_ssh(&second.directory, Some(&ssh));
    background::exchange(
        &first.directory,
        "start",
        json!({"rule_id":one.id}),
        Duration::from_secs(2),
    )
    .unwrap();
    let first_fixture = Path::new(first.ssh_config.as_ref().unwrap())
        .parent()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while state(&first, &one.id) != "ON" {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    std::fs::write(first_fixture.join("exit"), "interrupt owned fixture").unwrap();
    while state(&first, &one.id) != "RETRYING" {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    auto_open::set(&first.directory, &one.id, true).unwrap();
    let mut view = Session::start(temp.path(), &second.id);
    view.expect("already requested by First");
    assert_eq!(state(&second, &two.id), "OFF");
    assert!(
        !Path::new(second.ssh_config.as_ref().unwrap())
            .parent()
            .unwrap()
            .join("parent.pid")
            .exists()
    );
    view.close();
}

#[test]
fn foreground_automatic_open_is_scoped_to_its_machine_and_closes_owned_tunnel() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let first = fixture_machine(&catalog, "First", 1);
    let second = fixture_machine(&catalog, "Second", 1);
    let fixture_stop = FixtureStop::new(temp.path(), &[&first, &second]);
    for machine in [&first, &second] {
        let rule = &Store::load(&machine.directory).unwrap().settings.forwards[0];
        auto_open::set(&machine.directory, &rule.id, true).unwrap();
    }
    let mut view = Session::with_ssh(temp.path(), &first.id, true, Some(&ssh));
    view.expect_row("First", "ON");
    view.wait_for(|| fixture_stop.pids().len() >= 2);
    let pids = fixture_stop.pids();
    assert!(
        !Path::new(second.ssh_config.as_ref().unwrap())
            .parent()
            .unwrap()
            .join("parent.pid")
            .exists()
    );
    assert!(!second.directory.join("endpoint.json").exists());
    view.close();
    let deadline = Instant::now() + Duration::from_secs(4);
    while pids.iter().copied().any(fixture_running) {
        assert!(
            Instant::now() < deadline,
            "Foreground-owned parent or proxy survived view exit before fallback cleanup"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn owned_fixture_cleanup_after_forced_foreground_app_exit() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = fixture_machine(&catalog, "Forced", 1);
    let cleanup = FixtureStop::new(temp.path(), &[&machine]);
    let rule = Store::load(&machine.directory).unwrap().settings.forwards[0].clone();
    auto_open::set(&machine.directory, &rule.id, true).unwrap();
    let mut view = Session::with_ssh(temp.path(), &machine.id, true, Some(&ssh));
    view.expect_row("Forced", "ON");
    view.wait_for(|| cleanup.pids().len() >= 2);
    let pids = cleanup.pids();
    // Simulate assertion cleanup, not graceful Q: Rust product Drop cannot be
    // assumed to run after SIGKILL. The independent fixture protocol must work.
    drop(view);
    drop(cleanup);
    assert!(pids.into_iter().all(|pid| !fixture_running(pid)));
}

#[test]
fn edit_checkbox_on_off_cancel_and_f2_share_metadata_without_starting_controller() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let first = fixture_machine(&catalog, "EditFirst", 1);
    let second = fixture_machine(&catalog, "EditSecond", 1);
    let one = Store::load(&first.directory).unwrap().settings.forwards[0].clone();
    let before = std::fs::read(first.directory.join("forwards.json")).unwrap();
    let other = std::fs::read(second.directory.join("forwards.json")).unwrap();
    let mut view = Session::start(temp.path(), &first.id);
    view.expect("Saved connections");
    view._pair
        .master
        .resize(PtySize {
            rows: 18,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    view.screen.screen_mut().set_size(18, 80);
    view.send("e");
    view.expect("[ ] Open automatically");
    view.send("\t\t \r");
    view.wait_for(|| {
        auto_open::Preferences::load(&first.directory)
            .unwrap()
            .enabled(&one.id)
    });
    // The compact viewport hides the ordinary details panel. The save notice
    // is stable here: neither machine had any automatic work in this view.
    view.expect("Favorite saved. Automatic opening applies");
    assert_eq!(
        std::fs::read(first.directory.join("forwards.json")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(second.directory.join("forwards.json")).unwrap(),
        other
    );
    assert!(!first.directory.join("endpoint.json").exists());
    assert!(!second.directory.join("endpoint.json").exists());
    view.send("\x1bOQ"); // F2 still reads the same option.
    view.expect("[x] Open automatically");
    view.send(" \r");
    view.wait_for(|| {
        !auto_open::Preferences::load(&first.directory)
            .unwrap()
            .enabled(&one.id)
    });
    view.send("e");
    view.expect("[ ] Open automatically");
    view.send("\t\t \x1b");
    view.expect("Saved connections");
    assert!(
        !auto_open::Preferences::load(&first.directory)
            .unwrap()
            .enabled(&one.id)
    );
    view.close();
    let mut reopened = Session::start(temp.path(), &first.id);
    reopened.expect("Saved connections");
    assert!(!first.directory.join("endpoint.json").exists());
    assert!(!second.directory.join("endpoint.json").exists());
    std::fs::write(first.directory.join(auto_open::FILE), "broken").unwrap();
    reopened.send("e");
    reopened.expect("[unavailable] Open automatically");
    reopened.send("\r");
    reopened.expect("Automatic opening is unavailable");
    reopened.expect("Edit favorite");
    assert_eq!(
        std::fs::read_to_string(first.directory.join(auto_open::FILE)).unwrap(),
        "broken"
    );
    reopened.send("\x1b");
    reopened.close();
}

#[test]
fn edit_checkbox_preserves_running_pid_and_stale_error_keeps_input_in_dialog() {
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = fixture_machine(&catalog, "EditRunning", 1);
    let _fixture_stop = FixtureStop::new(temp.path(), &[&machine]);
    let _controller = Cleanup::with_ssh(&machine.directory, Some(&ssh));
    let rule = Store::load(&machine.directory).unwrap().settings.forwards[0].clone();
    background::call(&machine.directory, "start", json!({"rule_id":rule.id})).unwrap();
    let mut view = Session::start(temp.path(), &machine.id);
    view.expect_row("EditRunning", "ON");
    let pid = tunnel_pid(&machine);
    view.send("e");
    view.expect("[ ] Open automatically");
    view.send("\t\t \r");
    view.wait_for(|| {
        auto_open::Preferences::load(&machine.directory)
            .unwrap()
            .enabled(&rule.id)
    });
    view.expect("Open automatically: on");
    assert_eq!(tunnel_pid(&machine), pid);
    view.close();
    let mut view = Session::start(temp.path(), &machine.id);
    view.expect("already requested");
    assert_eq!(tunnel_pid(&machine), pid);
    view.send("e");
    view.expect("[x] Open automatically");
    // Name input is retained when another tab changes the option during Edit.
    view.send("\tMy retained name");
    auto_open::set(&machine.directory, &rule.id, false).unwrap();
    view.send("\r");
    view.expect("Automatic opening changed in another view");
    view.expect("Edit favorite");
    view.expect("My retained name");
    assert_eq!(
        Store::load(&machine.directory).unwrap().settings.forwards[0],
        rule
    );
    assert_eq!(tunnel_pid(&machine), pid);
    view.send("\x1b");
    view.expect("Saved connections");
    view.send("\r");
    view.wait_for(|| state(&machine, &rule.id) == "OFF");
    view.close();
    let mut reopened = Session::start(temp.path(), &machine.id);
    reopened.expect_row("EditRunning", "OFF");
    assert_eq!(state(&machine, &rule.id), "OFF");
    reopened.close();
}

#[test]
fn automatic_dispatch_rechecks_preferences_after_blocked_controller_preparation() {
    for change in ["disable", "favorite", "destination"] {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::new(temp.path()).unwrap();
        let machine = fixture_machine(&catalog, "Blocked", 1);
        let mut store = Store::load(&machine.directory).unwrap();
        let rule = store.settings.forwards[0].clone();
        auto_open::set(&machine.directory, &rule.id, true).unwrap();
        // The old implementation validated before its SECOND status call
        // (inside ensure_daemon), so an edit at this gate escaped validation.
        let peer =
            controlled_controller::Peer::with_occurrence(&machine.directory, "status", 2, &[], &[]);
        let mut view = Session::start(temp.path(), &machine.id);
        view.expect("Saved connections");
        peer.wait_blocked();
        match change {
            "disable" => auto_open::set(&machine.directory, &rule.id, false).unwrap(),
            "favorite" => {
                let mut edited = rule.clone();
                edited.remote_port += 1;
                store.save(vec![edited]).unwrap();
            }
            _ => {
                store.settings.host = "changed.invalid".into();
                store.save(vec![rule.clone()]).unwrap();
            }
        }
        peer.release();
        view.expect("automatic openings need attention");
        assert!(
            !peer
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(command, _)| command == "start")
        );
        view.close();
    }
}

#[test]
fn rapid_manual_stops_are_queued_and_automatic_failures_remain_inspectable() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = fixture_machine(&catalog, "Manual", 3);
    let rules = Store::load(&machine.directory).unwrap().settings.forwards;
    let ids = rules.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>();
    let peer = controlled_controller::Peer::start(&machine.directory, "stop", &ids, &[]);
    let mut view = Session::start(temp.path(), &machine.id);
    view.expect_row("Manual", "ON");
    view.send("\r");
    peer.wait_blocked();
    // Held Enter must coalesce the in-flight and pending IDs, while three
    // distinct rows each retain their Stop request.
    view.send("\r\r\x1b[B\r\r\r\x1b[B\r\r\r");
    view.send("\x1bOQ");
    view.expect("Manual / Manual 2"); // All repeated keys consumed before release.
    peer.release();
    view.send("\x1b");
    view.wait_for(|| {
        ids.iter().all(|id| {
            peer.states
                .lock()
                .unwrap()
                .get(id)
                .is_some_and(|state| state == "OFF")
        })
    });
    assert_eq!(
        peer.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(command, _)| command == "stop")
            .count(),
        3
    );
    view.close();
    drop(peer);
    for id in &ids {
        auto_open::set(&machine.directory, id, true).unwrap();
    }
    let peer = controlled_controller::Peer::start(&machine.directory, "", &[], &ids[..1]);
    let mut view = Session::start(temp.path(), &machine.id);
    view.wait_for(|| {
        peer.states
            .lock()
            .unwrap()
            .get(&ids[1])
            .is_some_and(|state| state == "ON")
    });
    view.expect("automatic openings need attention");
    view.expect("Fixture rejected this automatic start");
    view.send("\x1b[B\x1b[A");
    view.expect("Fixture rejected this automatic start");
    view.close();
}

#[cfg(windows)]
#[test]
#[ignore = "Explicit developer Python path; mandatory separate Windows CI step"]
fn native_new_view_automatic_open_uses_legacy_python_controller() {
    let python = std::path::PathBuf::from(
        std::env::var_os("PORTS_LEGACY_PYTHON")
            .expect("Set PORTS_LEGACY_PYTHON to the developer Python executable"),
    );
    assert!(python.is_absolute() && python.is_file());
    let temp = tempfile::tempdir().unwrap();
    let ssh = fixture_ssh(temp.path());
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = fixture_machine(&catalog, "Legacy", 1);
    let _fixture_stop = FixtureStop::new(temp.path(), &[&machine]);
    let rule = Store::load(&machine.directory).unwrap().settings.forwards[0].clone();
    let original = std::fs::read(machine.directory.join("forwards.json")).unwrap();
    auto_open::set(&machine.directory, &rule.id, true).unwrap();
    let _legacy = Cleanup::legacy(&machine.directory, &python, &ssh);
    let mut first = Session::start(temp.path(), &machine.id);
    first.expect_row("Legacy", "ON");
    let pid = tunnel_pid(&machine);
    let mut second = Session::start(temp.path(), &machine.id);
    second.expect("already requested");
    assert_eq!(tunnel_pid(&machine), pid);
    first.send("\r");
    first.wait_for(|| state(&machine, &rule.id) == "OFF");
    first.close();
    second.close();
    let mut reopened = Session::start(temp.path(), &machine.id);
    reopened.expect_row("Legacy", "ON");
    assert_ne!(tunnel_pid(&machine), pid);
    assert_eq!(
        std::fs::read(machine.directory.join("forwards.json")).unwrap(),
        original
    );
    reopened.close();
}
