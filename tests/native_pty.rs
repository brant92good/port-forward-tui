//! Real ConPTY/PTY keyboard tests. No desktop windows or personal data.
use port_forward_tui::{background, machines::Catalog, store::Store};
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
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env_remove("WT_SESSION");
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
impl Drop for Session {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
    }
}
struct Cleanup {
    path: std::path::PathBuf,
    child: std::process::Child,
}
impl Cleanup {
    fn start(path: &Path) -> Self {
        // portable-pty deliberately owns its child in a non-breakaway Windows
        // job. Start the shared controller outside that harness job; this test
        // qualifies keyboard/multiple views, not desktop launcher detachment.
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ports"));
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
