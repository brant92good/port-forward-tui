//! Buffered output through an actual owned PTY, including modal and exit paths.
use port_forward_tui::{
    machines::Catalog,
    store::{Forward, Store},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Child(Box<dyn portable_pty::Child + Send + Sync>);
impl Drop for Child {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !matches!(self.0.try_wait(), Ok(Some(_))) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn buffered_frames_survive_modals_resize_and_restore_screen_on_exit() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let machine = catalog
        .add("render-check.invalid", "Render check", None, None)
        .unwrap();
    Store::load(&machine.directory)
        .unwrap()
        .save(vec![
            Forward::new(28001, 8000, "Owned render favorite").unwrap(),
        ])
        .unwrap();
    let original = std::fs::read(machine.directory.join("forwards.json")).unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_ports"));
    command.args([
        "--data-dir",
        temp.path().to_str().unwrap(),
        "--machine",
        &machine.id,
    ]);
    command.cwd(temp.path());
    command.env("TERM", "xterm-256color");
    command.env_remove("WT_SESSION");
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut input = pair.master.take_writer().unwrap();
    let (send, receive) = mpsc::channel();
    let mut child = Child(pair.slave.spawn_command(command).unwrap());
    drop(pair.slave);
    thread::spawn(move || {
        let mut bytes = [0; 8192];
        while let Ok(count) = reader.read(&mut bytes) {
            if count == 0 || send.send(bytes[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = vt100::Parser::new(32, 120, 0);
    let mut transcript = Vec::new();
    let mut query = Vec::new();
    let mut pump = |screen: &mut vt100::Parser, input: &mut Box<dyn Write + Send>| {
        if let Ok(bytes) = receive.recv_timeout(Duration::from_millis(30)) {
            screen.process(&bytes);
            transcript.extend_from_slice(&bytes);
            query.extend_from_slice(&bytes);
            if query.windows(4).any(|part| part == b"\x1b[6n") {
                input.write_all(b"\x1b[1;1R").unwrap();
                input.flush().unwrap();
                query.clear();
            } else if query.len() > 3 {
                query.drain(..query.len() - 3);
            }
        }
    };
    let mut wait_text = |screen: &mut vt100::Parser,
                         input: &mut Box<dyn Write + Send>,
                         text: &str,
                         absent: &str| {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            pump(screen, input);
            let content = screen.screen().contents();
            if content.contains(text) && (absent.is_empty() || !content.contains(absent)) {
                break;
            }
            assert!(Instant::now() < deadline, "Missing {text:?}: {content}");
        }
    };
    wait_text(&mut screen, &mut input, "Owned render favorite", "");
    assert!(screen.screen().alternate_screen());
    for (key, title) in [
        ("?", "Ports keyboard"),
        ("\x1bOQ", "Settings"),
        ("e", "Edit favorite"),
    ] {
        input.write_all(key.as_bytes()).unwrap();
        input.flush().unwrap();
        wait_text(&mut screen, &mut input, title, "");
        input.write_all(b"\x1b").unwrap();
        input.flush().unwrap();
        wait_text(&mut screen, &mut input, "Q close", title);
    }
    pair.master
        .resize(PtySize {
            rows: 24,
            cols: 90,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    screen.screen_mut().set_size(24, 90);
    wait_text(&mut screen, &mut input, "Owned render favorite", "");
    pair.master
        .resize(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    screen.screen_mut().set_size(32, 120);
    // A new modal after resize proves a fresh draw, not merely old buffer text.
    input.write_all(b"?").unwrap();
    input.flush().unwrap();
    wait_text(&mut screen, &mut input, "Ports keyboard", "");
    input.write_all(b"\x1b").unwrap();
    input.flush().unwrap();
    wait_text(&mut screen, &mut input, "Q close", "Ports keyboard");
    input.write_all(b"q").unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump(&mut screen, &mut input);
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "Owned app did not exit");
    }
    while let Ok(bytes) = receive.recv_timeout(Duration::from_millis(80)) {
        screen.process(&bytes);
        transcript.extend_from_slice(&bytes);
    }
    assert!(
        !screen.screen().alternate_screen(),
        "Alternate screen not restored"
    );
    assert!(!screen.screen().hide_cursor(), "Cursor not restored");
    let leave = b"\x1b[?1049l";
    let after = transcript
        .windows(leave.len())
        .rposition(|p| p == leave)
        .expect("No leave-screen sequence")
        + leave.len();
    assert!(!String::from_utf8_lossy(&transcript[after..]).contains("Owned render favorite"));
    assert_eq!(
        std::fs::read(machine.directory.join("forwards.json")).unwrap(),
        original
    );
    assert!(!machine.directory.join("endpoint.json").exists());
}
