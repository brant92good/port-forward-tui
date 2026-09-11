use super::*;
use crate::{machines::Machine, store::Settings};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::AtomicUsize,
    time::Instant,
};

struct Server {
    port: u16,
    count: Arc<AtomicUsize>,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(response: Vec<u8>, delay: Duration) -> Self {
        Self::with_prefix(Vec::new(), response, delay)
    }
    fn with_prefix(prefix: Vec<u8>, response: Vec<u8>, delay: Duration) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (counter, records, stopped) = (count.clone(), requests.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0; 1024];
                while data.len() < 4096 && !data.windows(4).any(|b| b == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => data.extend_from_slice(&buffer[..n]),
                    }
                }
                counter.fetch_add(1, Ordering::AcqRel);
                records
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&data).into_owned());
                let _ = stream.write_all(&prefix);
                let until = Instant::now() + delay;
                while Instant::now() < until && !stopped.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(2));
                }
                let _ = stream.write_all(&response);
            }
        });
        Self {
            port,
            count,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn html(html: &str) -> Self {
        Self::new(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",html.len()).into_bytes(),Duration::ZERO)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn entry(directory: &std::path::Path, port: u16, name: &str) -> Entry {
    let rule = Forward::new(port, 8000, name).unwrap();
    let mut store = Store {
        directory: directory.into(),
        settings: Settings {
            host: "fixture".into(),
            ..Settings::default()
        },
    };
    store.save(vec![rule.clone()]).unwrap();
    Entry {
        machine: Machine {
            id: "machine-fixture".into(),
            name: "Fixture".into(),
            target: "fixture".into(),
            directory: directory.into(),
            ssh_port: None,
            ssh_config: None,
        },
        rule: Some(rule),
        state: "ON".into(),
        details: String::new(),
        open_automatically: false,
    }
}
fn complete(inspector: &mut Inspector, entry: &Entry) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while inspector.job.is_some() {
        inspector.poll(std::slice::from_ref(entry));
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn real_http_title_uses_only_selected_loopback_root_and_decodes_text() {
    let server = Server::html(
        "<!doctype html><title>開發 &amp; Studio</title><img src='http://example.invalid/trap'>",
    );
    assert_eq!(
        probe(server.port, &AtomicBool::new(false)).unwrap(),
        "開發 & Studio"
    );
    assert_eq!(server.count.load(Ordering::Acquire), 1);
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].starts_with("GET / HTTP/1.1\r\n"));
    assert!(!requests[0].to_ascii_lowercase().contains("authorization:"));
    assert!(!requests[0].to_ascii_lowercase().contains("cookie:"));
}

#[test]
fn custom_and_port_literal_names_require_explicit_view_choice_and_never_write() {
    let server = Server::html("<title>Fixture Studio</title>");
    for name in ["My custom API", "Port 8000"] {
        let root = tempfile::tempdir().unwrap();
        let entry = entry(root.path(), server.port, name);
        let bytes = std::fs::read(root.path().join("forwards.json")).unwrap();
        let mut inspector = Inspector::default();
        inspector.poll(std::slice::from_ref(&entry));
        assert!(inspector.label(&entry).is_none());
        inspector.open(&entry, false).unwrap();
        complete(&mut inspector, &entry);
        assert!(inspector.label(&entry).is_none());
        assert!(matches!(
            inspector.preview.as_ref().unwrap().state,
            State::Ready(_)
        ));
        inspector.key(key(KeyCode::Char('u')));
        assert_eq!(inspector.label(&entry), Some("Fixture Studio"));
        inspector.reset(&entry);
        assert!(inspector.label(&entry).is_none());
        assert_eq!(
            std::fs::read(root.path().join("forwards.json")).unwrap(),
            bytes
        );
        let mut files = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files, [".ports-channel", "forwards.json"]);
    }
}

#[test]
fn offline_or_cancelled_checks_send_nothing_and_empty_model_does_no_work() {
    let server = Server::html("<title>Trap</title>");
    let root = tempfile::tempdir().unwrap();
    let mut entry = entry(root.path(), server.port, "API");
    let mut inspector = Inspector::default();
    inspector.poll(std::slice::from_ref(&entry));
    for state in ["OFF", "UNKNOWN", "CONNECTING", "RETRYING"] {
        entry.state = state.into();
        assert!(inspector.open(&entry, false).is_err());
    }
    assert!(probe(server.port, &AtomicBool::new(true)).is_err());
    assert_eq!(server.count.load(Ordering::Acquire), 0);
}

#[test]
fn stale_result_changed_binding_custom_name_or_stop_cannot_be_adopted() {
    let server=Server::new(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 20\r\n\r\n<title>Stale</title>".to_vec(),Duration::from_millis(80));
    let root = tempfile::tempdir().unwrap();
    let entry = entry(root.path(), server.port, "API");
    let mut inspector = Inspector::default();
    inspector.open(&entry, false).unwrap();
    let mut changed = entry.clone();
    changed.rule.as_mut().unwrap().name = "New custom".into();
    complete(&mut inspector, &changed);
    inspector.key(key(KeyCode::Char('u')));
    assert!(inspector.label(&changed).is_none());
    assert!(matches!(
        inspector.preview.as_ref().unwrap().state,
        State::Failed(_)
    ));
    inspector.close();
    for change in ["port", "route", "stop"] {
        let frozen = Frozen::from_entry(&entry).unwrap();
        let mut changed = entry.clone();
        match change {
            "port" => changed.rule.as_mut().unwrap().local_port += 1,
            "route" => changed.machine.target = "other".into(),
            _ => changed.state = "OFF".into(),
        }
        assert!(!frozen.matches(&changed));
    }
}

#[test]
fn redirects_unsupported_pages_and_title_spoofs_keep_fallback() {
    let trap = Server::html("<title>Must not visit</title>");
    let redirect = Server::new(
        format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/\r\nContent-Length: 0\r\n\r\n",
            trap.port
        )
        .into_bytes(),
        Duration::ZERO,
    );
    assert!(probe(redirect.port, &AtomicBool::new(false)).is_err());
    assert_eq!(trap.count.load(Ordering::Acquire), 0);
    for html in [
        "<!-- <title>Wrong</title> -->",
        "<script>let x='<title>Wrong</title>';</script>",
        "<svg><title>Wrong</title></svg>",
        "<title>\u{1b}[31mBad</title>",
        "<title>\u{202e}Bad</title>",
        "<title>\u{061c}Bad</title>",
        "<title>\u{200b}\u{200d}</title>",
        "<title> </title>",
    ] {
        assert!(title(html).is_err(), "{html}");
    }
    assert!(title(&format!("<title>{}</title>", "a".repeat(81))).is_err());
    let no_title = Server::html("<h1>No title</h1>");
    assert!(probe(no_title.port, &AtomicBool::new(false)).is_err());
    let binary = Server::new(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
        Duration::ZERO,
    );
    assert!(probe(binary.port, &AtomicBool::new(false)).is_err());
}

#[test]
fn chunked_response_and_size_time_limits_use_real_sockets() {
    let body = "<title>Chunked Studio</title>";
    let chunked=Server::new(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n{body}\r\n0\r\n\r\n",body.len()).into_bytes(),Duration::ZERO);
    assert_eq!(
        probe(chunked.port, &AtomicBool::new(false)).unwrap(),
        "Chunked Studio"
    );
    let large = Server::html(&format!(
        "<title>Too large</title>{}",
        "x".repeat(BODY_LIMIT as usize)
    ));
    assert!(probe(large.port, &AtomicBool::new(false)).is_err());
    let header = Server::new(
        format!(
            "HTTP/1.1 200 OK\r\nX-Huge: {}\r\n\r\n",
            "x".repeat(HEADER_LIMIT + 1)
        )
        .into_bytes(),
        Duration::ZERO,
    );
    assert!(probe(header.port, &AtomicBool::new(false)).is_err());
    let stalled = Server::new(Vec::new(), Duration::from_secs(3));
    let started = Instant::now();
    assert!(probe(stalled.port, &AtomicBool::new(false)).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn close_discards_worker_result_and_repeated_open_never_queues() {
    let server=Server::new(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 19\r\n\r\n<title>Late</title>".to_vec(),Duration::from_millis(120));
    let root = tempfile::tempdir().unwrap();
    let entry = entry(root.path(), server.port, "API");
    let mut inspector = Inspector::default();
    inspector.open(&entry, false).unwrap();
    assert!(inspector.open(&entry, false).is_err());
    inspector.close();
    complete(&mut inspector, &entry);
    assert!(inspector.preview.is_none() && inspector.label(&entry).is_none());
    assert!(server.count.load(Ordering::Acquire) <= 1);
}

#[test]
fn body_deadline_and_unsupported_auth_encoding_responses_are_bounded() {
    let stalled_body = Server::with_prefix(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n<title>\r\n".to_vec(),
        Vec::new(), Duration::from_secs(3));
    let started = Instant::now();
    assert!(probe(stalled_body.port, &AtomicBool::new(false)).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(stalled_body.count.load(Ordering::Acquire), 1);
    for response in [
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=fixture\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1\r\n\r\n\xff".as_slice(),
    ] {
        let server = Server::new(response.to_vec(), Duration::ZERO);
        assert!(probe(server.port, &AtomicBool::new(false)).is_err());
        assert_eq!(server.count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn compact_preview_shows_both_choices_source_and_view_only_limit() {
    use ratatui::{Terminal, backend::TestBackend};
    let root = tempfile::tempdir().unwrap();
    let row = entry(root.path(), 8000, "Custom API");
    let inspector = Inspector {
        preview: Some(Preview {
            frozen: Frozen::from_entry(&row).unwrap(),
            state: State::Ready("Development Studio".into()),
        }),
        job: None,
        labels: BTreeMap::new(),
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    terminal.draw(|frame| inspector.draw(frame)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    for expected in [
        "Custom API",
        "127.0.0.1:8000",
        "Development Studio",
        "Enter: keep saved name",
        "U: use web title in this view",
        "This display choice is not saved",
        "Esc cancels",
    ] {
        assert!(
            text.contains(expected),
            "Missing {expected} from compact dialog: {text}"
        );
    }
}
