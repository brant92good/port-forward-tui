//! Compatible protocol-1 controller. Views may come and go; this process owns SSH.
use crate::{
    forwarding::Manager,
    process::{Backend, NativeBackend},
    store::{self, Forward, Lock, Store},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL: u8 = 1;
const MAX_REQUEST: usize = 65_536;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol: u8,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

fn read_line(stream: &mut TcpStream, limit: usize, timeout: Duration) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut chunk = [0_u8; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "Controller message timed out.");
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut chunk)?;
        ensure!(count != 0, "Incomplete controller message.");
        let end = chunk[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| offset + 1);
        bytes.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        ensure!(bytes.len() <= limit, "Oversized controller message.");
        if end.is_some() {
            return Ok(bytes);
        }
    }
}

pub fn exchange(
    directory: &Path,
    command: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<Value> {
    let bytes =
        fs::read(directory.join("endpoint.json")).context("No background manager is available.")?;
    ensure!(bytes.len() <= 8192, "Invalid controller endpoint.");
    let endpoint: Endpoint = serde_json::from_slice(&bytes)?;
    ensure!(
        endpoint.protocol == PROTOCOL && endpoint.port > 0 && endpoint.token.len() == 64,
        "Incompatible controller endpoint."
    );
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut connection = TcpStream::connect_timeout(&address, timeout.min(Duration::from_secs(2)))?;
    connection.set_read_timeout(Some(timeout))?;
    connection.set_write_timeout(Some(timeout))?;
    let mut request = arguments.as_object().cloned().unwrap_or_default();
    request.insert("protocol".into(), json!(PROTOCOL));
    request.insert("token".into(), json!(endpoint.token));
    request.insert("command".into(), json!(command));
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_REQUEST,
        "Controller request is too large."
    );
    connection.write_all(&bytes)?;
    let response: Value =
        serde_json::from_slice(&read_line(&mut connection, MAX_RESPONSE, timeout)?)?;
    ensure!(
        response.get("ok") == Some(&Value::Bool(true)),
        "{}",
        response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Background manager rejected the request.")
    );
    Ok(response)
}

pub struct Supervisor<B: Backend> {
    pub store: Store,
    pub manager: Manager<B>,
    token: String,
    pub stopping: bool,
}

impl<B: Backend> Supervisor<B> {
    pub fn new(store: Store, backend: B) -> Self {
        Self {
            manager: Manager::new(store.settings.clone(), backend),
            store,
            token: format!("{}{}", store::new_id(), store::new_id()),
            stopping: false,
        }
    }
    pub fn snapshot(&self) -> Value {
        let now = Instant::now();
        let states = self.manager.states();
        let details: BTreeMap<_, _> = states
            .keys()
            .map(|id| (id.clone(), self.manager.details(id, now)))
            .collect();
        json!({"ok":true,"protocol":PROTOCOL,"pid":std::process::id(),
            "host":self.manager.settings.host,"ssh_port":self.manager.settings.ssh_port,"ssh_config":self.manager.settings.ssh_config,
            "capabilities":["shared_favorites","auto_reconnect"],"forwards":self.store.settings.forwards,
            "running":self.manager.running(),"states":states,"details":details})
    }
    pub fn local(&mut self, command: &str, arguments: Value) -> Result<Value> {
        let mut request = arguments;
        request["command"] = json!(command);
        request["protocol"] = json!(PROTOCOL);
        request["token"] = json!(self.token);
        self.dispatch(&request)
    }
    fn authenticate(&self, request: &Value) -> Result<()> {
        let provided = request
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .as_bytes();
        ensure!(provided.len() == self.token.len(), "Unauthorized");
        let different = provided
            .iter()
            .zip(self.token.bytes())
            .fold(0_u8, |bits, (left, right)| bits | (left ^ right));
        ensure!(different == 0, "Unauthorized");
        ensure!(
            request.get("protocol") == Some(&json!(PROTOCOL)),
            "Incompatible background protocol; restart the background manager."
        );
        Ok(())
    }
    pub fn dispatch(&mut self, request: &Value) -> Result<Value> {
        self.authenticate(request)?;
        ensure!(
            !self.stopping,
            "Background manager is stopping. Reopen after it exits."
        );
        let action = request
            .get("command")
            .and_then(Value::as_str)
            .context("Missing controller command.")?;
        // Stop works even with damaged JSON or an externally changed destination.
        if !matches!(action, "stop" | "stop_all" | "shutdown") {
            let current = Store::load(&self.store.directory)?;
            self.manager.check_destination(&current.settings)?;
            self.store = current;
        }
        let mut result_rule = None;
        let now = Instant::now();
        match action {
            "status" => {}
            "start" => {
                let id = request
                    .get("rule_id")
                    .and_then(Value::as_str)
                    .context("Missing favorite ID.")?;
                let rule = self
                    .store
                    .settings
                    .forwards
                    .iter()
                    .find(|rule| rule.id == id)
                    .context("Save the forward before starting it.")?;
                self.manager.start(rule, now)?;
            }
            "upsert" => {
                let mut rule: Forward = serde_json::from_value(
                    request.get("rule").context("Missing favorite.")?.clone(),
                )?;
                rule.validate()?;
                ensure!(!rule.name.is_empty(), "A favorite needs a name.");
                let current = self
                    .store
                    .settings
                    .forwards
                    .iter()
                    .find(|saved| saved.id == rule.id)
                    .cloned();
                let expected = request.get("expected").filter(|value| !value.is_null());
                if let Some(expected) = expected {
                    ensure!(
                        current
                            .as_ref()
                            .is_some_and(
                                |saved| serde_json::to_value(saved).ok().as_ref() == Some(expected)
                            ),
                        "This favorite changed in another view. Reopen Edit and try again."
                    );
                } else if let Some(current) = &current {
                    ensure!(
                        current == &rule,
                        "This favorite changed in another view. Refresh and try again."
                    );
                }
                let duplicate = self
                    .store
                    .settings
                    .forwards
                    .iter()
                    .find(|saved| {
                        saved.id != rule.id
                            && saved.local_port == rule.local_port
                            && saved.remote_port == rule.remote_port
                    })
                    .cloned();
                if let Some(duplicate) = duplicate {
                    ensure!(
                        current.is_none(),
                        "That mapping is already saved in another favorite."
                    );
                    rule = duplicate;
                } else {
                    let mut updated = self.store.settings.forwards.clone();
                    if let Some(saved) = updated.iter_mut().find(|saved| saved.id == rule.id) {
                        *saved = rule.clone();
                    } else {
                        updated.push(rule.clone());
                    }
                    self.store.save(updated)?;
                    if current.as_ref().is_some_and(|saved| saved != &rule)
                        && self.manager.state(&rule.id).requested()
                    {
                        self.manager.restart(&rule, now)?;
                    }
                }
                if request.get("start") == Some(&Value::Bool(true)) {
                    self.manager.start(&rule, now)?;
                }
                result_rule = Some(rule.id);
            }
            "delete" => {
                let id = request
                    .get("rule_id")
                    .and_then(Value::as_str)
                    .context("Missing favorite ID.")?;
                if let Some(current) = self
                    .store
                    .settings
                    .forwards
                    .iter()
                    .find(|rule| rule.id == id)
                {
                    ensure!(
                        request.get("expected") == Some(&serde_json::to_value(current)?),
                        "This favorite changed in another view. Select it again before deleting."
                    );
                    let updated = self
                        .store
                        .settings
                        .forwards
                        .iter()
                        .filter(|rule| rule.id != id)
                        .cloned()
                        .collect();
                    self.store.save(updated)?;
                    self.manager.stop(id);
                }
            }
            "stop" => self.manager.stop(
                request
                    .get("rule_id")
                    .and_then(Value::as_str)
                    .context("Invalid favorite ID.")?,
            ),
            "stop_all" | "shutdown" => {
                self.manager.close();
                self.stopping = action == "shutdown";
            }
            _ => anyhow::bail!("Unknown controller command."),
        }
        self.manager.poll(now);
        let mut snapshot = self.snapshot();
        if let Some(id) = result_rule {
            snapshot["rule_id"] = json!(id);
        }
        Ok(snapshot)
    }
}

struct EndpointGuard {
    path: PathBuf,
    token: String,
}
impl Drop for EndpointGuard {
    fn drop(&mut self) {
        if fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Endpoint>(&bytes).ok())
            .is_some_and(|endpoint| endpoint.token == self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn serve(directory: &Path) -> Result<()> {
    let directory = store::absolute(directory)?;
    let _lock = Lock::acquire(&directory, "daemon.lock", Duration::ZERO)?;
    let store = Store::load(&directory)?;
    store::host(&store.settings.host)?;
    let supervisor = Supervisor::new(store, NativeBackend::new()?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let endpoint = Endpoint {
        protocol: PROTOCOL,
        pid: std::process::id(),
        port: listener.local_addr()?.port(),
        token: supervisor.token.clone(),
    };
    let endpoint_path = directory.join("endpoint.json");
    store::write_json(&endpoint_path, &endpoint)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&endpoint_path, fs::Permissions::from_mode(0o600))?;
    }
    let _endpoint = EndpointGuard {
        path: endpoint_path,
        token: endpoint.token,
    };
    let supervisor = Arc::new(Mutex::new(supervisor));
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::Relaxed);
    })?;
    let clients = Arc::new(AtomicUsize::new(0));
    let mut next_poll = Instant::now();
    loop {
        {
            let mut supervisor = supervisor
                .lock()
                .map_err(|_| anyhow::anyhow!("Controller state lock failed."))?;
            if supervisor.stopping || interrupted.load(Ordering::Relaxed) {
                supervisor.manager.close();
                break;
            }
            if Instant::now() >= next_poll {
                supervisor.manager.poll(Instant::now());
                next_poll = Instant::now()
                    + Duration::from_millis(if cfg!(target_os = "macos") { 750 } else { 250 });
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                if clients.load(Ordering::Relaxed) >= 16 {
                    continue;
                }
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                clients.fetch_add(1, Ordering::Relaxed);
                let clients = Arc::clone(&clients);
                let supervisor = Arc::clone(&supervisor);
                thread::spawn(move || {
                    struct Count(Arc<AtomicUsize>);
                    impl Drop for Count {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                    let _count = Count(clients);
                    let result = (|| -> Result<Value> {
                        let request: Value = serde_json::from_slice(&read_line(
                            &mut stream,
                            MAX_REQUEST,
                            Duration::from_secs(2),
                        )?)?;
                        let mut supervisor = supervisor
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Controller state lock failed."))?;
                        supervisor.dispatch(&request)
                    })();
                    let response = result
                        .unwrap_or_else(|error| json!({"ok":false,"error":format!("{error:#}")}));
                    if let Ok(mut bytes) = serde_json::to_vec(&response) {
                        if bytes.len() >= MAX_RESPONSE {
                            bytes = br#"{"ok":false,"error":"Controller response exceeded its size limit."}"#.to_vec();
                        }
                        bytes.push(b'\n');
                        let _ = stream.write_all(&bytes);
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Let the shutdown response finish before the process returns. A slow peer
    // cannot hold the process indefinitely; all socket operations have deadlines.
    let deadline = Instant::now() + Duration::from_secs(3);
    while clients.load(Ordering::Relaxed) != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

pub fn launch(directory: &Path) -> Result<Child> {
    let directory = store::absolute(directory)?;
    fs::create_dir_all(&directory)?;
    let path = directory.join("port_forward_tui.background.log");
    if path.metadata().is_ok_and(|data| data.len() > 256 * 1024) {
        // A concurrent launcher may already have rotated the log. Logging must
        // never replace the daemon lock as the authority on who owns tunnels.
        let _ = fs::rename(
            &path,
            directory.join("port_forward_tui.background.previous.log"),
        );
    }
    let log = OpenOptions::new().create(true).append(true).open(path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--serve", "--data-dir"])
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    crate::process::configure_daemon(&mut command);
    command.spawn().context(
        "Could not detach the background manager. Launch Ports from a normal terminal session.",
    )
}

pub fn ensure_daemon(directory: &Path) -> Result<Value> {
    if let Ok(snapshot) = exchange(directory, "status", json!({}), Duration::from_secs(2)) {
        return Ok(snapshot);
    }
    let mut child = launch(directory)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Ok(snapshot) = exchange(directory, "status", json!({}), Duration::from_millis(500)) {
            // Reap our losing launcher if it exited. Never terminate the winner.
            let _ = child.try_wait();
            return Ok(snapshot);
        }
        let _ = child.try_wait();
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!(
        "Background manager did not start. See {}",
        directory.join("port_forward_tui.background.log").display()
    )
}

pub fn call(directory: &Path, command: &str, arguments: Value) -> Result<Value> {
    if !matches!(command, "stop" | "stop_all" | "shutdown")
        || !directory.join("endpoint.json").exists()
    {
        ensure_daemon(directory)?;
    }
    exchange(directory, command, arguments, Duration::from_secs(5))
}

pub fn restart(directory: &Path) -> Result<Vec<String>> {
    let snapshot = match exchange(directory, "status", json!({}), Duration::from_secs(2)) {
        Ok(snapshot) => snapshot,
        Err(_) if !directory.join("endpoint.json").exists() => {
            ensure_daemon(directory)?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let wanted = snapshot["forwards"]
        .as_array()
        .context("Invalid controller favorites.")?
        .iter()
        .filter_map(|rule| rule["id"].as_str())
        .filter(|id| {
            matches!(
                snapshot["states"][id].as_str(),
                Some("ON" | "CONNECTING" | "RETRYING")
            )
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    exchange(directory, "shutdown", json!({}), Duration::from_secs(5))?;
    // Lock release proves the old controller relinquished ownership. Avoid PID
    // reuse guesses and never force-kill a controller during an upgrade.
    let lock = Lock::acquire(directory, "daemon.lock", Duration::from_secs(8))
        .context("The old manager has not exited; it was not force-killed.")?;
    drop(lock);
    ensure_daemon(directory)?;
    for id in &wanted {
        exchange(
            directory,
            "start",
            json!({"rule_id":id}),
            Duration::from_secs(5),
        )?;
    }
    Ok(wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process::{Listeners, TunnelProcess},
        store::Settings,
    };
    struct NeverSsh;
    impl Backend for NeverSsh {
        fn start(&mut self, _: &Settings, _: &Forward) -> Result<Box<dyn TunnelProcess>> {
            anyhow::bail!("Isolated controller test")
        }
        fn listeners(&mut self, _: &[u32]) -> Result<Listeners> {
            Ok(Default::default())
        }
    }
    fn request(supervisor: &Supervisor<NeverSsh>, command: &str, arguments: Value) -> Value {
        let mut request = arguments;
        request["token"] = json!(supervisor.token);
        request["protocol"] = json!(PROTOCOL);
        request["command"] = json!(command);
        request
    }
    #[test]
    fn stale_concurrent_edits_and_deletes_cannot_overwrite_newer_favorites() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::load(temp.path()).unwrap();
        store.settings.host = "workbox".into();
        store.save(vec![]).unwrap();
        let mut supervisor = Supervisor::new(store, NeverSsh);
        let first = Forward::new(18000, 8000, "API").unwrap();
        let req = request(&supervisor, "upsert", json!({"rule":first,"expected":null}));
        supervisor.dispatch(&req).unwrap();
        let mut edited = first.clone();
        edited.name = "Renamed API".into();
        let req = request(
            &supervisor,
            "upsert",
            json!({"rule":edited,"expected":first}),
        );
        supervisor.dispatch(&req).unwrap();
        let req = request(
            &supervisor,
            "upsert",
            json!({"rule":first,"expected":first}),
        );
        assert!(supervisor.dispatch(&req).is_err());
        let req = request(
            &supervisor,
            "delete",
            json!({"rule_id":first.id,"expected":first}),
        );
        assert!(supervisor.dispatch(&req).is_err());
        assert_eq!(
            Store::load(temp.path()).unwrap().settings.forwards,
            [edited]
        );
        let duplicate = Forward::new(18000, 8000, "Second name").unwrap();
        let req = request(&supervisor, "upsert", json!({"rule":duplicate}));
        let snapshot = supervisor.dispatch(&req).unwrap();
        assert_eq!(snapshot["rule_id"], first.id);
        assert_eq!(snapshot["forwards"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn authentication_fails_before_mutation_and_stop_survives_broken_json() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::load(temp.path()).unwrap();
        store.settings.host = "workbox".into();
        store.save(vec![]).unwrap();
        let mut supervisor = Supervisor::new(store, NeverSsh);
        assert!(
            supervisor
                .dispatch(&json!({"protocol":1,"token":"bad","command":"shutdown"}))
                .is_err()
        );
        assert!(!supervisor.stopping);
        fs::write(temp.path().join("forwards.json"), b"broken").unwrap();
        let req = request(&supervisor, "status", json!({}));
        assert!(supervisor.dispatch(&req).is_err());
        let req = request(&supervisor, "stop_all", json!({}));
        assert!(supervisor.dispatch(&req).is_ok());
        let req = request(&supervisor, "shutdown", json!({}));
        assert!(supervisor.dispatch(&req).is_ok());
        assert!(supervisor.stopping);
    }
}
