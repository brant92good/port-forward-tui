//! Hidden ConPTY/PTY first-frame measurement; no live data, SSH or desktop window.
//! Usage: cargo run --release --example render_probe -- BASELINE [CANDIDATE]
use anyhow::{Context, Result, ensure};
use port_forward_tui::{
    machines::Catalog,
    store::{Forward, Store},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default, serde::Serialize)]
struct WriteStats {
    writes: usize,
    bytes: usize,
    flushes: usize,
    #[serde(skip)]
    contents: Vec<u8>,
}
struct Meter<W> {
    inner: W,
    stats: std::rc::Rc<std::cell::RefCell<WriteStats>>,
}
impl<W: Write> Write for Meter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        let mut stats = self.stats.borrow_mut();
        stats.writes += 1;
        stats.bytes += count;
        stats.contents.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stats.borrow_mut().flushes += 1;
        self.inner.flush()
    }
}

fn frame_child(mode: &str, root: &Path, id: &str, report: &Path) -> Result<()> {
    use crossterm::{
        execute,
        terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use port_forward_tui::{
        screen::Input,
        ui::{self, Entry, Presentation},
    };
    use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(
                std::io::stderr(),
                LeaveAlternateScreen,
                crossterm::cursor::Show
            );
        }
    }
    terminal::enable_raw_mode()?;
    let _restore = Restore;
    execute!(std::io::stderr(), EnterAlternateScreen)?;
    let (columns, height) = terminal::size()?;
    let machine = Catalog::new(root)?.get(id)?;
    let rows = Store::load(&machine.directory)?
        .settings
        .forwards
        .into_iter()
        .map(|rule| Entry {
            machine: machine.clone(),
            rule: Some(rule),
            state: "UNKNOWN".into(),
            details: String::new(),
            open_automatically: false,
        })
        .collect::<Vec<_>>();
    let stats = std::rc::Rc::new(std::cell::RefCell::new(WriteStats::default()));
    let measured = Meter {
        inner: std::io::stderr(),
        stats: stats.clone(),
    };
    let writer: Box<dyn Write> = match mode {
        "raw" => Box::new(measured),
        "buffer" => Box::new(port_forward_tui::screen::FrameWriter::new(measured)),
        _ => anyhow::bail!("Unknown writer mode"),
    };
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(writer),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, columns, height)),
        },
    )?;
    let quick = Input::default();
    let started = Instant::now();
    terminal.draw(|frame| {
        ui::render(
            frame,
            &Presentation {
                rows: &rows,
                selected: 0,
                quick: &quick,
                typing: false,
                notice: "",
                busy: false,
                persistent: true,
                automatic: &Default::default(),
                automatic_errors: &Default::default(),
            },
        )
    })?;
    let draw_ms = started.elapsed().as_secs_f64() * 1000.0;
    std::fs::write(
        report,
        serde_json::to_vec(&json!({"draw_ms":draw_ms,"writer":*stats.borrow(),
        "frame_sha256":format!("{:x}",Sha256::digest(&stats.borrow().contents))}))?,
    )?;
    let mut key = [0];
    std::io::stdin().read_exact(&mut key)?;
    ensure!(key == *b"q", "Unexpected probe input");
    Ok(())
}

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

fn sample(
    binary: &Path,
    columns: u16,
    rows: u16,
    writer: Option<&str>,
) -> Result<serde_json::Value> {
    let temp = tempfile::tempdir()?;
    let catalog = Catalog::new(temp.path())?;
    let machine = catalog.add("render-probe.invalid", "Render probe", None, None)?;
    let rules = (0..40)
        .map(|index| Forward::new(20000 + index, 20000 + index, &format!("Probe {index:02}")))
        .collect::<Result<Vec<_>>>()?;
    Store::load(&machine.directory)?.save(rules)?;
    let pair = native_pty_system().openpty(PtySize {
        rows,
        cols: columns,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(binary);
    let report = temp.path().join("writer-report.json");
    if let Some(mode) = writer {
        command.args([
            "--frame-child",
            mode,
            temp.path().to_str().unwrap(),
            &machine.id,
            report.to_str().unwrap(),
        ]);
    } else {
        command.args([
            "--data-dir",
            temp.path().to_str().unwrap(),
            "--machine",
            &machine.id,
        ]);
    }
    command.cwd(temp.path());
    command.env("TERM", "xterm-256color");
    command.env_remove("WT_SESSION");
    let mut reader = pair.master.try_clone_reader()?;
    let mut input = pair.master.take_writer()?;
    let (send, receive) = mpsc::channel();
    let started = Instant::now();
    let mut child = Child(pair.slave.spawn_command(command)?);
    let reader_thread = thread::spawn(move || {
        let mut bytes = [0; 8192];
        while let Ok(count) = reader.read(&mut bytes) {
            if count == 0
                || send
                    .send((started.elapsed(), bytes[..count].to_vec()))
                    .is_err()
            {
                break;
            }
        }
    });
    drop(pair.slave);
    let mut parser = vt100::Parser::new(rows, columns, 0);
    let mut pending_query = Vec::new();
    let mut first = None;
    let mut complete = None;
    let mut chunks = 0;
    let mut bytes = 0;
    let deadline = started + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Ok((when, chunk)) = receive.recv_timeout(Duration::from_millis(50)) {
            chunks += 1;
            bytes += chunk.len();
            parser.process(&chunk);
            pending_query.extend_from_slice(&chunk);
            if pending_query.windows(4).any(|part| part == b"\x1b[6n") {
                input.write_all(b"\x1b[1;1R")?;
                input.flush()?;
                pending_query.clear();
            } else if pending_query.len() > 3 {
                pending_query.drain(..pending_query.len() - 3);
            }
            let content = parser.screen().contents();
            if first.is_none() && content.contains("PORTS") {
                first = Some(when);
            }
            if content.contains("Saved connections")
                && content.contains("Probe 00")
                && content.contains("Open automatically: off")
                && content.contains("Q close")
            {
                complete = Some(when);
                break;
            }
        }
        ensure!(
            child.0.try_wait()?.is_none(),
            "App exited before complete frame"
        );
    }
    let complete =
        complete.with_context(|| format!("No complete frame: {}", parser.screen().contents()))?;
    let first = first.context("Complete frame lacked initial title")?;
    input.write_all(b"q")?;
    input.flush()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait()? {
            ensure!(status.success(), "App failed: {status:?}");
            break;
        }
        ensure!(Instant::now() < deadline, "App did not close");
        while receive.try_recv().is_ok() {}
        thread::sleep(Duration::from_millis(5));
    }
    drop(input);
    drop(pair.master);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !reader_thread.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    ensure!(
        reader_thread.is_finished(),
        "Owned PTY reader did not close"
    );
    reader_thread.join().expect("PTY reader panicked");
    ensure!(
        !machine.directory.join("endpoint.json").exists(),
        "Render probe started controller"
    );
    let mut result = json!({"columns":columns,"rows":rows,"spawn_to_title_ms":first.as_secs_f64()*1000.0,
        "spawn_to_full_ms":complete.as_secs_f64()*1000.0,"title_to_full_ms":(complete-first).as_secs_f64()*1000.0,
        "received_chunks":chunks,"received_bytes":bytes});
    if writer.is_some() {
        result["frame_writer"] = serde_json::from_slice(&std::fs::read(report)?)?;
    }
    Ok(result)
}

fn main() -> Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().is_some_and(|a| a == "--frame-child") {
        ensure!(arguments.len() == 5, "Expected owned frame child arguments");
        return frame_child(
            &arguments[1],
            Path::new(&arguments[2]),
            &arguments[3],
            Path::new(&arguments[4]),
        );
    }
    let writers = arguments == ["--writers"];
    let binaries = if writers {
        vec![std::env::current_exe()?, std::env::current_exe()?]
    } else {
        arguments
            .iter()
            .map(std::path::PathBuf::from)
            .map(std::fs::canonicalize)
            .collect::<std::io::Result<Vec<_>>>()?
    };
    ensure!(
        !binaries.is_empty() && binaries.len() <= 2,
        "Expected BASELINE [CANDIDATE]"
    );
    let mut samples = Vec::new();
    for (columns, rows) in [(120, 32), (220, 65)] {
        for round in 0..7 {
            // Alternate order across rounds; record warm-up separately.
            for offset in 0..binaries.len() {
                let index = (round + offset) % binaries.len();
                let mut result = sample(
                    &binaries[index],
                    columns,
                    rows,
                    writers.then_some(if index == 0 { "raw" } else { "buffer" }),
                )?;
                result["variant"] = json!(index);
                result["round"] = json!(round);
                result["warmup"] = json!(round == 0);
                samples.push(result);
            }
        }
    }
    let hashes = binaries
        .iter()
        .map(|p| std::fs::read(p).map(|b| format!("{:x}", Sha256::digest(b))))
        .collect::<std::io::Result<Vec<_>>>()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"kind":"hidden-pty-byte-arrival",
        "physical_paint_measured":false,"same_layout_writer_comparison":writers,"binary_sha256":hashes,"samples":samples}))?
    );
    Ok(())
}
