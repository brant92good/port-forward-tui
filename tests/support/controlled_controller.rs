//! Test-only protocol-1 peer for deterministic blocked-preparation/Stop tests.
//! No process or SSH is spawned. Production controller/process tests are separate.
use port_forward_tui::{
    background::Endpoint,
    store::{self, Store},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub struct Peer {
    release: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    arrived: mpsc::Receiver<()>,
    pub calls: Arc<Mutex<Vec<(String, String)>>>,
    pub states: Arc<Mutex<BTreeMap<String, String>>>,
    trace: Arc<Mutex<Vec<String>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Peer {
    pub fn start(directory: &Path, blocked: &'static str, on: &[String], fail: &[String]) -> Self {
        Self::with_occurrence(directory, blocked, 1, on, fail, None)
    }
    pub fn change_on_status(
        directory: &Path,
        occurrence: usize,
        change: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self::with_occurrence(
            directory,
            "status",
            occurrence,
            &[],
            &[],
            Some(Box::new(change)),
        )
    }
    fn with_occurrence(
        directory: &Path,
        blocked: &'static str,
        occurrence: usize,
        on: &[String],
        fail: &[String],
        mut change: Option<Box<dyn FnOnce() + Send>>,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = Endpoint {
            protocol: 1,
            pid: std::process::id(),
            port: listener.local_addr().unwrap().port(),
            token: format!("{}{}", store::new_id(), store::new_id()),
        };
        store::write_json(&directory.join("endpoint.json"), &endpoint).unwrap();
        let directory = directory.to_path_buf();
        let release = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let states = Arc::new(Mutex::new(
            on.iter()
                .map(|id| (id.clone(), "ON".into()))
                .collect::<BTreeMap<_, _>>(),
        ));
        let fail = fail.iter().cloned().collect::<BTreeSet<_>>();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let trace_worker = trace.clone();
        let (sender, arrived) = mpsc::channel();
        let (released, stopped, recorded, current) =
            (release.clone(), quit.clone(), calls.clone(), states.clone());
        let worker = thread::spawn(move || {
            let started = Instant::now();
            let note = |event: String| {
                trace_worker
                    .lock()
                    .unwrap()
                    .push(format!("{} ms: {event}", started.elapsed().as_millis()));
            };
            let mut matching_calls = 0;
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut line = String::new();
                if BufReader::new(&mut stream).read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                assert_eq!(request["token"], endpoint.token);
                let command = request["command"].as_str().unwrap().to_owned();
                let id = request["rule_id"].as_str().unwrap_or("").to_owned();
                recorded.lock().unwrap().push((command.clone(), id.clone()));
                note(format!("received {command} {id}"));
                if command == blocked {
                    matching_calls += 1;
                }
                if command == blocked && matching_calls == occurrence {
                    if let Some(change) = change.take() {
                        note(format!("status {occurrence}: mutation begin"));
                        change();
                        note(format!("status {occurrence}: mutation complete"));
                        sender.send(()).unwrap();
                    } else {
                        sender.send(()).unwrap();
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !released.load(Ordering::Relaxed)
                            && !stopped.load(Ordering::Relaxed)
                            && Instant::now() < deadline
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        note(format!(
                            "gate ended: released={}",
                            released.load(Ordering::Relaxed)
                        ));
                    }
                }
                let failed = command == "start" && fail.contains(&id);
                if !failed {
                    if command == "start" {
                        current.lock().unwrap().insert(id.clone(), "ON".into());
                    }
                    if command == "stop" {
                        current.lock().unwrap().insert(id.clone(), "OFF".into());
                    }
                }
                let settings = Store::load(&directory).unwrap().settings;
                let response = if failed {
                    json!({"ok":false,"error":"Fixture rejected this automatic start"})
                } else {
                    json!({"ok":true,"protocol":1,"pid":endpoint.pid,"host":settings.host,
                        "forwards":settings.forwards,"states":*current.lock().unwrap(),"details":{},"running":[]})
                };
                let result = writeln!(stream, "{response}");
                note(format!("responded {command}: {result:?}"));
            }
        });
        Self {
            release,
            quit,
            arrived,
            calls,
            states,
            trace,
            worker: Some(worker),
        }
    }
    pub fn wait_blocked(&self) {
        self.arrived
            .recv_timeout(Duration::from_secs(8))
            .expect("Request did not reach fixture gate");
    }
    pub fn release(&self) {
        self.release.store(true, Ordering::Relaxed);
    }
    pub fn wait_changed(&self) {
        assert!(
            self.arrived.recv_timeout(Duration::from_secs(8)).is_ok(),
            "Mutation did not complete: {}",
            self.trace()
        );
    }
    pub fn trace(&self) -> String {
        self.trace.lock().unwrap().join("\n")
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.release();
        self.quit.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
