#![cfg(target_os = "linux")]
//! Real PTY hangup and SIGTERM cleanup; only an SSH-shaped owned fixture runs.
use port_forward_tui::{
    machines::Catalog,
    process,
    store::{Forward, Store},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}
fn running(pid: u32) -> bool {
    // A dead orphan may briefly remain in /proc while init reaps it. It cannot
    // execute or own listeners, so distinguish that from a surviving proxy.
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .is_some_and(|stat| {
            stat.rsplit_once(')').is_some_and(|(_, fields)| {
                !matches!(fields.split_whitespace().next(), Some("Z" | "X"))
            })
        })
}
struct FixtureCleanup {
    directory: PathBuf,
    executable: PathBuf,
}
impl FixtureCleanup {
    fn kill_owned_group(&self, signal: i32) {
        let Some(parent) = pid(&self.directory.join("parent.pid")) else {
            return;
        };
        let config = self.directory.join("ssh_config");
        let child_file = self.directory.join("child.pid");
        let owns = |candidate: u32, argument: &Path| {
            fs::read_link(format!("/proc/{candidate}/exe")).is_ok_and(|exe| exe == self.executable)
                && fs::read(format!("/proc/{candidate}/cmdline")).is_ok_and(|args| {
                    args.split(|byte| *byte == 0)
                        .any(|arg| arg == argument.as_os_str().as_encoded_bytes())
                })
                && unsafe { libc::getpgid(candidate as i32) } == parent as i32
        };
        // A still-running parent or proxy must match this unique compiled
        // fixture and its exact config/PID-file argument before signaling.
        if owns(parent, &config) || pid(&child_file).is_some_and(|child| owns(child, &child_file)) {
            unsafe {
                libc::kill(-(parent as i32), signal);
            }
        }
    }
}
impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        self.kill_owned_group(libc::SIGTERM);
        thread::sleep(Duration::from_millis(100));
        self.kill_owned_group(libc::SIGKILL);
    }
}
struct Terminal {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pair: Option<portable_pty::PtyPair>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    parser: vt100::Parser,
    pending: Vec<u8>,
}
fn process_diagnostic(pid: u32) -> String {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let wait = fs::read_to_string(format!("/proc/{pid}/wchan")).unwrap_or_default();
    let descriptors = fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            fs::read_link(entry.path())
                .ok()
                .map(|path| format!("{}={}", entry.file_name().to_string_lossy(), path.display()))
        })
        .collect::<Vec<_>>();
    format!("pid={pid} stat={stat:?} wchan={wait:?} fds={descriptors:?}")
}
impl Terminal {
    fn start(root: &Path, machine: &str, bin: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 110,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let fd = pair.master.as_raw_fd().unwrap();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            assert!(flags >= 0);
            assert_eq!(libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK), 0);
        }
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_ports"));
        command.args([
            "--foreground",
            "--data-dir",
            root.to_str().unwrap(),
            "--machine",
            machine,
        ]);
        command.env("TERM", "xterm-256color");
        command.env_remove("WT_SESSION");
        command.env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
        let child = pair.slave.spawn_command(command).unwrap();
        let reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        Self {
            child,
            pair: Some(pair),
            reader: Some(reader),
            writer: Some(writer),
            parser: vt100::Parser::new(30, 110, 0),
            pending: Vec::new(),
        }
    }
    fn pump(&mut self) {
        let mut bytes = [0; 8192];
        if let Some(reader) = &mut self.reader {
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                self.parser.process(&bytes[..count]);
                self.pending.extend_from_slice(&bytes[..count]);
            }
        }
        if let Some(offset) = self.pending.windows(4).position(|part| part == b"\x1b[6n") {
            self.pending.drain(..offset + 4);
            self.send("\x1b[1;1R");
        }
    }
    fn send(&mut self, text: &str) {
        self.writer
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        self.writer.as_mut().unwrap().flush().unwrap();
    }
    fn wait(&mut self, stage: &str, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition(self) {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "{stage}: {}",
                self.parser.screen().contents()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn hangup(&mut self) {
        // portable-pty's writer Drop normally injects newline+EOF. In raw mode
        // that newline could toggle the selected tunnel OFF and hide a missing
        // hangup handler. Suppress that test-harness write before closing FDs.
        let fd = self.pair.as_ref().unwrap().master.as_raw_fd().unwrap();
        unsafe {
            let mut state = std::mem::zeroed::<libc::termios>();
            assert_eq!(libc::tcgetattr(fd, &mut state), 0);
            state.c_cc[libc::VEOF] = 0;
            assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &state), 0);
        }
        eprintln!(
            "before HUP {}",
            process_diagnostic(self.child.process_id().unwrap())
        );
        eprintln!(
            "before HUP owner {}",
            process_diagnostic(std::process::id())
        );
        // Every master descriptor must close for a real terminal hangup.
        self.writer.take();
        self.reader.take();
        self.pair.take();
        eprintln!("after HUP owner {}", process_diagnostic(std::process::id()));
    }
    fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.child.try_wait().unwrap().is_none() {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "Foreground app survived termination: {}",
                process_diagnostic(self.child.process_id().unwrap())
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn check_foreground_cleanup(hangup: bool) {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let build = Command::new("rustc")
        .arg("--edition=2024")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/ssh_fixture.rs"))
        .arg("-o")
        .arg(bin.join("ssh"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    {
        let root = temp.path().join(if hangup { "hangup" } else { "sigterm" });
        let fixture = root.join("fixture");
        fs::create_dir_all(&fixture).unwrap();
        let config = fixture.join("ssh_config");
        fs::write(&config, b"# owned test fixture").unwrap();
        let _cleanup = FixtureCleanup {
            directory: fixture.clone(),
            executable: fs::canonicalize(bin.join("ssh")).unwrap(),
        };
        let catalog = Catalog::new(&root).unwrap();
        let machine = catalog
            .add("fixture.invalid", "Fixture", None, Some(&config))
            .unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let rule = Forward::new(port, 8000, "Hangup fixture").unwrap();
        Store::load(&machine.directory)
            .unwrap()
            .save(vec![rule])
            .unwrap();
        let mut terminal = Terminal::start(&root, &machine.id, &bin);
        terminal.wait("main screen", |terminal| {
            terminal
                .parser
                .screen()
                .contents()
                .contains("Saved connections")
        });
        terminal.send("\r");
        terminal.wait("owned SSH and proxy started", |_| {
            pid(&fixture.join("parent.pid")).is_some_and(|pid| {
                process::owned_listeners(&[pid]).is_ok_and(|ports| ports.contains(&(pid, port)))
            }) && pid(&fixture.join("child.pid")).is_some()
        });
        let parent = pid(&fixture.join("parent.pid")).unwrap();
        let proxy = pid(&fixture.join("child.pid")).unwrap();
        if hangup {
            terminal.hangup();
        } else {
            assert_eq!(
                unsafe { libc::kill(terminal.child.process_id().unwrap() as i32, libc::SIGTERM) },
                0
            );
        }
        terminal.wait_exit();
        terminal.wait("owned SSH and proxy cleaned up", |_| {
            !running(parent) && !running(proxy)
        });
        assert!(process::owned_listeners(&[parent]).unwrap().is_empty());
        assert!(!machine.directory.join("endpoint.json").exists());
    }
}

#[test]
fn foreground_pty_close_stops_owned_ssh_and_proxy() {
    check_foreground_cleanup(true);
}

#[test]
fn foreground_sigterm_stops_owned_ssh_and_proxy() {
    check_foreground_cleanup(false);
}
