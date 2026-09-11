//! Hidden ConPTY/PTY and actual HTTP requests; no desktop or personal hosts.
#[allow(dead_code)]
#[path = "support/controlled_controller.rs"]
mod controlled_controller;
use port_forward_tui::{
    background,
    machines::Catalog,
    store::{Forward, Store},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct Http {
    port: u16,
    count: Arc<AtomicUsize>,
    delay: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Http {
    fn start() -> Self {
        Self::at(0)
    }
    fn at(port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let delay = Arc::new(AtomicU64::new(0));
        let (counter, stopped, wait) = (count.clone(), stop.clone(), delay.clone());
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 1024];
                while bytes.len() < 4096 && !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => bytes.extend_from_slice(&buf[..n]),
                    }
                }
                counter.fetch_add(1, Ordering::AcqRel);
                assert!(bytes.starts_with(b"GET / HTTP/1.1\r\n"));
                let until = Instant::now() + Duration::from_millis(wait.load(Ordering::Acquire));
                while Instant::now() < until && !stopped.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(2));
                }
                let body = "<!doctype html><title>Fixture Studio</title><h1>Owned web app</h1>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            port,
            count,
            delay,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
struct View {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pair: portable_pty::PtyPair,
    input: Box<dyn Write + Send>,
    output: mpsc::Receiver<Vec<u8>>,
    screen: vt100::Parser,
}
impl View {
    fn start(root: &Path, machine: &str, proxy: u16) -> Self {
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
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, format!("http://127.0.0.1:{proxy}"));
        }
        command.env("NO_PROXY", "");
        command.env("no_proxy", "");
        let child = pair.slave.spawn_command(command).unwrap();
        let input = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (send, output) = mpsc::sync_channel(64);
        thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || send.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            _pair: pair,
            input,
            output,
            screen: vt100::Parser::new(32, 130, 0),
        }
    }
    fn send(&mut self, text: &str) {
        self.input.write_all(text.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }
    fn pump(&mut self) {
        if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(20)) {
            if bytes.windows(4).any(|b| b == b"\x1b[6n") {
                self.send("\x1b[1;1R");
            }
            self.screen.process(&bytes);
        }
    }
    fn wait(&mut self, mut condition: impl FnMut(&str) -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            let text = self.screen.screen().contents();
            if condition(&text) {
                return;
            }
            assert!(Instant::now() < until, "Unexpected screen: {text}");
        }
    }
    fn expect(&mut self, text: &str) {
        self.wait(|screen| screen.contains(text));
    }
    fn row(&mut self, name: &str) {
        self.wait(|screen| {
            screen
                .lines()
                .any(|line| line.contains('›') && line.contains(name))
        });
    }
    fn settle(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.pump();
        }
    }
    fn close(&mut self) {
        self.send("q");
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(Instant::now() < until);
            self.pump();
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            self.pump();
        }
    }
}

fn setup(
    root: &Path,
    config: Option<&str>,
    local: u16,
    remote: u16,
) -> (port_forward_tui::machines::Machine, Forward) {
    let catalog = Catalog::new(root).unwrap();
    let machine = catalog
        .add("fixture", "Web fixture", None, config.map(Path::new))
        .unwrap();
    let rule = Forward::new(local, remote, "My custom API").unwrap();
    Store::load(&machine.directory)
        .unwrap()
        .save(vec![rule.clone()])
        .unwrap();
    (machine, rule)
}
fn exercise(view: &mut View, http: &Http, proxy: &Http) {
    view.row("My custom API");
    view.settle(Duration::from_millis(900));
    assert_eq!(
        http.count.load(Ordering::Acquire),
        0,
        "Opening/polling must not inspect HTTP"
    );
    view.send("t");
    view.expect("Web page title: Fixture Studio");
    assert_eq!(http.count.load(Ordering::Acquire), 1);
    view.send("\r");
    view.row("My custom API");
    view.send("t");
    view.expect("Web page title: Fixture Studio");
    view.send("u");
    view.row("Fixture Studio");
    assert_eq!(http.count.load(Ordering::Acquire), 2);
    assert_eq!(
        proxy.count.load(Ordering::Acquire),
        0,
        "Proxy environment must never route the title request"
    );
    view.send("e");
    view.expect("Edit favorite");
    view.send("\x1b");
    view.row("My custom API");
}

#[test]
fn owned_view_title_preview_choice_custom_reset_no_startup_probe_or_controller_write() {
    let temp = tempfile::tempdir().unwrap();
    let http = Http::start();
    let proxy = Http::start();
    let (machine, rule) = setup(temp.path(), None, http.port, 8000);
    let peer = controlled_controller::Peer::start(
        &machine.directory,
        "never",
        std::slice::from_ref(&rule.id),
        &[],
    );
    let before = std::fs::read(machine.directory.join("forwards.json")).unwrap();
    let mut view = View::start(temp.path(), &machine.id, proxy.port);
    exercise(&mut view, &http, &proxy);
    http.delay.store(1100, Ordering::Release);
    view.send("t");
    view.expect("Reading one HTTP page");
    let until = Instant::now() + Duration::from_secs(1);
    while http.count.load(Ordering::Acquire) < 3 {
        assert!(Instant::now() < until);
        view.pump();
    }
    peer.states
        .lock()
        .unwrap()
        .insert(rule.id.clone(), "OFF".into());
    view.expect("changed or stopped");
    view.send("u\x1b");
    view.row("My custom API");
    view.send("t");
    // The canceled socket may still be within its deadline; either state
    // rejects a new request. Neither notice means another probe was queued.
    view.wait(|text| {
        text.contains("Start this forward") || text.contains("previous check is finishing")
    });
    view.close();
    assert_eq!(
        std::fs::read(machine.directory.join("forwards.json")).unwrap(),
        before
    );
    assert!(
        peer.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(command, _)| command == "status")
    );
    assert_eq!(http.count.load(Ordering::Acquire), 3);
}

struct Shutdown {
    path: PathBuf,
    child: std::process::Child,
}
impl Shutdown {
    fn start(path: &Path) -> Self {
        // This fixture qualifies actual title traffic and unchanged SSH state.
        // Desktop breakaway is separate: tool/ConPTY jobs prohibit detachment.
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
        let mut owner = Self {
            path: path.into(),
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while background::exchange(path, "status", json!({}), Duration::from_millis(200)).is_err() {
            assert!(
                owner.child.try_wait().unwrap().is_none(),
                "Owned controller exited before readiness"
            );
            assert!(Instant::now() < deadline, "Owned controller did not start");
            thread::sleep(Duration::from_millis(20));
        }
        owner
    }
}
impl Drop for Shutdown {
    fn drop(&mut self) {
        let _ = background::exchange(&self.path, "shutdown", json!({}), Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(4);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
#[ignore = "Requires explicit owned portable OpenSSH fixture config, never personal SSH"]
fn actual_ssh_forward_http_title_does_not_change_controller_or_saved_forward() {
    let config = std::env::var("PORTS_TEST_SSH_CONFIG").expect("PORTS_TEST_SSH_CONFIG");
    let http_port: u16 = std::env::var("PORTS_TEST_HTTP_PORT")
        .expect("PORTS_TEST_HTTP_PORT must match the owned sshd PermitOpen port")
        .parse()
        .unwrap();
    assert_ne!(http_port, 0);
    let temp = tempfile::tempdir().unwrap();
    let http = Http::at(http_port);
    let proxy = Http::start();
    let socket = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let (machine, rule) = setup(temp.path(), Some(&config), port, http.port);
    let mut owner = Shutdown::start(&machine.directory);
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ports"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW; owned CLI only.
    }
    let output = command
        .args([
            "--data-dir",
            temp.path().to_str().unwrap(),
            "--machine",
            &machine.id,
            "start",
            &rule.id,
            "--wait",
            "5",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let before = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    let saved = std::fs::read(machine.directory.join("forwards.json")).unwrap();
    let mut view = View::start(temp.path(), &machine.id, proxy.port);
    exercise(&mut view, &http, &proxy);
    view.close();
    let after = background::exchange(
        &machine.directory,
        "status",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(after["pid"], before["pid"]);
    assert_eq!(after["states"][&rule.id], "ON");
    assert_eq!(
        std::fs::read(machine.directory.join("forwards.json")).unwrap(),
        saved
    );
    background::exchange(
        &machine.directory,
        "shutdown",
        json!({}),
        Duration::from_secs(2),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while machine.directory.join("endpoint.json").exists() {
        assert!(
            Instant::now() < deadline,
            "Owned controller did not remove its endpoint"
        );
        thread::sleep(Duration::from_millis(20));
    }
    while owner.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "Owned controller did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(100)
        )
        .is_err(),
        "Owned SSH listener survived shutdown"
    );
}
