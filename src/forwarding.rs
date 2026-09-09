//! Tunnel intent and recovery are owned by the controller, not by a TUI view.
use crate::{
    process::{Backend, TunnelProcess},
    store::{Forward, Settings},
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Off,
    Connecting,
    On,
    Retrying,
    Error,
}
impl State {
    pub fn requested(self) -> bool {
        matches!(self, Self::Connecting | Self::On | Self::Retrying)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Connecting => "CONNECTING",
            Self::On => "ON",
            Self::Retrying => "RETRYING",
            Self::Error => "ERROR",
        }
    }
}

struct Tunnel {
    rule: Forward,
    state: State,
    process: Option<Box<dyn TunnelProcess>>,
    started: Instant,
    connected: Option<Instant>,
    retry_at: Option<Instant>,
    attempts: u32,
    details: String,
}

pub struct Manager<B: Backend> {
    pub settings: Settings,
    backend: B,
    tunnels: BTreeMap<String, Tunnel>,
}

impl<B: Backend> Manager<B> {
    pub fn new(settings: Settings, backend: B) -> Self {
        Self {
            settings,
            backend,
            tunnels: BTreeMap::new(),
        }
    }
    pub fn state(&self, id: &str) -> State {
        self.tunnels.get(id).map_or(State::Off, |t| t.state)
    }
    pub fn states(&self) -> BTreeMap<String, State> {
        self.tunnels
            .iter()
            .map(|(id, t)| (id.clone(), t.state))
            .collect()
    }
    pub fn running(&self) -> Vec<String> {
        self.tunnels
            .iter()
            .filter(|(_, t)| t.process.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn details(&self, id: &str, now: Instant) -> String {
        let Some(tunnel) = self.tunnels.get(id) else {
            return String::new();
        };
        let mut text = tunnel.details.clone();
        if let Some(process) = &tunnel.process {
            let recent = process.details();
            if !recent.is_empty() {
                text.push('\n');
                text.push_str(&recent);
            }
        }
        if let Some(retry_at) = tunnel.retry_at {
            let seconds = retry_at.saturating_duration_since(now).as_secs_f64().ceil() as u64;
            text.push_str(&format!("\nNetwork connection interrupted. Retrying in {seconds}s. Enter stops retries; R retries now."));
        }
        let start = text
            .char_indices()
            .rev()
            .nth(5999)
            .map_or(0, |(offset, _)| offset);
        text[start..].trim().to_owned()
    }
    pub fn start(&mut self, rule: &Forward, now: Instant) -> Result<()> {
        rule.validate()?;
        if self.state(&rule.id).requested() {
            return Ok(());
        }
        self.tunnels.insert(
            rule.id.clone(),
            Tunnel {
                rule: rule.clone(),
                state: State::Connecting,
                process: None,
                started: now,
                connected: None,
                retry_at: None,
                attempts: 0,
                details: String::new(),
            },
        );
        self.attempt(&rule.id, now);
        Ok(())
    }
    fn attempt(&mut self, id: &str, now: Instant) {
        let rule = self.tunnels[id].rule.clone();
        let conflict = self
            .tunnels
            .iter()
            .find(|(other, t)| {
                other.as_str() != id && t.state.requested() && t.rule.local_port == rule.local_port
            })
            .map(|(_, t)| t.rule.name.clone());
        let result = if let Some(name) = conflict {
            Err(anyhow::anyhow!(
                "Local port {} is requested by {name}. Choose another local port.",
                rule.local_port
            ))
        } else {
            self.backend.start(&self.settings, &rule)
        };
        let tunnel = self
            .tunnels
            .get_mut(id)
            .expect("Attempt owns an existing tunnel");
        tunnel.retry_at = None;
        tunnel.connected = None;
        tunnel.started = now;
        tunnel.details.clear();
        match result {
            Ok(process) => {
                tunnel.process = Some(process);
                tunnel.state = State::Connecting;
            }
            Err(error) => {
                tunnel.state = State::Error;
                tunnel.details = format!("{error:#}");
            }
        }
    }
    pub fn stop(&mut self, id: &str) {
        if let Some(tunnel) = self.tunnels.get_mut(id) {
            if let Some(mut process) = tunnel.process.take() {
                process.stop();
                let details = process.details();
                if !details.is_empty() {
                    tunnel.details = details;
                }
            }
            tunnel.state = State::Off;
            tunnel.retry_at = None;
            tunnel.connected = None;
            tunnel.attempts = 0;
        }
    }
    pub fn close(&mut self) {
        for id in self.tunnels.keys().cloned().collect::<Vec<_>>() {
            self.stop(&id);
        }
    }
    pub fn restart(&mut self, rule: &Forward, now: Instant) -> Result<()> {
        self.stop(&rule.id);
        self.start(rule, now)
    }
    pub fn check_destination(&self, settings: &Settings) -> Result<()> {
        ensure!(
            self.settings.same_destination(settings),
            "SSH destination changed. Restore it and add a separate machine instead."
        );
        Ok(())
    }
    fn failed(tunnel: &mut Tunnel, reason: &str, now: Instant) {
        if let Some(mut process) = tunnel.process.take() {
            process.stop();
            tunnel.details = process.details();
        }
        tunnel.details.push('\n');
        tunnel.details.push_str(reason);
        tunnel.connected = None;
        let details = tunnel.details.to_lowercase();
        let permanent = [
            "permission denied",
            "host key verification failed",
            "remote host identification has changed",
            "bad configuration option",
            "bad owner or permissions",
            "no such identity",
            "unknown option",
            "bad port",
            "could not open user configuration file",
            "cannot listen to port",
            "address already in use",
            "administratively prohibited",
        ];
        if permanent.iter().any(|text| details.contains(text)) {
            tunnel.state = State::Error;
            tunnel.retry_at = None;
            tunnel
                .details
                .push_str("\nFix the SSH or local-port error, then press Enter to retry.");
        } else {
            let delay = (1_u64 << (tunnel.attempts.saturating_add(1).min(5))).min(30);
            tunnel.attempts = tunnel.attempts.saturating_add(1);
            tunnel.retry_at = Some(now + Duration::from_secs(delay));
            tunnel.state = State::Retrying;
        }
    }
    pub fn poll(&mut self, now: Instant) {
        let pids = self
            .tunnels
            .values()
            .filter_map(|t| t.process.as_ref().map(|p| p.id()))
            .collect::<Vec<_>>();
        let listeners = self.backend.listeners(&pids);
        for tunnel in self.tunnels.values_mut() {
            let Some(process) = tunnel.process.as_mut() else {
                continue;
            };
            let pid = process.id();
            match process.exited() {
                Ok(Some(code)) => {
                    Self::failed(tunnel, &format!("SSH exited with code {code}."), now);
                    continue;
                }
                Err(error) => {
                    Self::failed(
                        tunnel,
                        &format!("Could not read SSH process state: {error}"),
                        now,
                    );
                    continue;
                }
                Ok(None) => {}
            }
            match &listeners {
                Ok(owned) if owned.contains(&(pid, tunnel.rule.local_port)) => {
                    tunnel.state = State::On;
                    let connected = tunnel.connected.get_or_insert(now);
                    if now.saturating_duration_since(*connected) >= Duration::from_secs(30) {
                        tunnel.attempts = 0;
                    }
                    tunnel.details.clear();
                }
                Ok(_)
                    if now.saturating_duration_since(tunnel.started) >= Duration::from_secs(20) =>
                {
                    Self::failed(
                        tunnel,
                        "SSH did not open its local port before the connection deadline.",
                        now,
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    // An OS query failure must not turn an unrelated listener into
                    // a success or kill a working tunnel. The UI reports the gap.
                    tunnel.details = format!("Cannot verify local listener ownership: {error:#}");
                }
            }
        }
        let ready = self
            .tunnels
            .iter()
            .filter(|(_, t)| t.state == State::Retrying && t.retry_at.is_some_and(|due| now >= due))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ready {
            self.attempt(&id, now);
        }
    }
}
impl<B: Backend> Drop for Manager<B> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Listeners;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct FakeState {
        spawned: u32,
        stopped: Vec<u32>,
        exited: BTreeMap<u32, (i32, String)>,
        owned: Listeners,
        probe_error: bool,
    }
    struct FakeBackend(Arc<Mutex<FakeState>>);
    struct FakeProcess {
        id: u32,
        state: Arc<Mutex<FakeState>>,
        stopped: bool,
    }
    impl TunnelProcess for FakeProcess {
        fn id(&self) -> u32 {
            self.id
        }
        fn exited(&mut self) -> Result<Option<i32>> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .exited
                .get(&self.id)
                .map(|(code, _)| *code))
        }
        fn stop(&mut self) {
            if !self.stopped {
                self.state.lock().unwrap().stopped.push(self.id);
                self.stopped = true;
            }
        }
        fn details(&self) -> String {
            self.state
                .lock()
                .unwrap()
                .exited
                .get(&self.id)
                .map(|(_, text)| text.clone())
                .unwrap_or_default()
        }
    }
    impl Backend for FakeBackend {
        fn start(&mut self, _: &Settings, _: &Forward) -> Result<Box<dyn TunnelProcess>> {
            let mut state = self.0.lock().unwrap();
            state.spawned += 1;
            Ok(Box::new(FakeProcess {
                id: state.spawned,
                state: Arc::clone(&self.0),
                stopped: false,
            }))
        }
        fn listeners(&mut self, _: &[u32]) -> Result<Listeners> {
            let state = self.0.lock().unwrap();
            ensure!(!state.probe_error, "probe unavailable");
            Ok(state.owned.clone())
        }
    }
    fn fixture() -> (
        Manager<FakeBackend>,
        Arc<Mutex<FakeState>>,
        Forward,
        Instant,
    ) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        (
            Manager::new(
                Settings {
                    host: "workbox".into(),
                    ..Default::default()
                },
                FakeBackend(Arc::clone(&state)),
            ),
            state,
            Forward::new(18000, 8000, "API").unwrap(),
            Instant::now(),
        )
    }
    #[test]
    fn only_owned_listener_means_on_and_off_is_not_an_intent_to_start() {
        let (mut manager, state, rule, now) = fixture();
        manager.poll(now);
        assert_eq!(state.lock().unwrap().spawned, 0);
        manager.start(&rule, now).unwrap();
        state.lock().unwrap().owned.insert((999, 18000));
        manager.poll(now);
        assert_eq!(manager.state(&rule.id), State::Connecting);
        state.lock().unwrap().owned.insert((1, 18000));
        manager.poll(now);
        assert_eq!(manager.state(&rule.id), State::On);
        manager.stop(&rule.id);
        manager.poll(now + Duration::from_secs(999));
        assert_eq!(state.lock().unwrap().spawned, 1);
        assert_eq!(manager.state(&rule.id), State::Off);
    }
    #[test]
    fn retry_is_bounded_and_stop_cancels_it_while_other_tunnels_remain_on() {
        let (mut manager, state, rule, now) = fixture();
        let other = Forward::new(18888, 8888, "Notebook").unwrap();
        manager.start(&rule, now).unwrap();
        manager.start(&other, now).unwrap();
        state.lock().unwrap().owned.insert((2, 18888));
        state
            .lock()
            .unwrap()
            .exited
            .insert(1, (255, "Connection reset by peer".into()));
        manager.poll(now);
        assert_eq!(manager.state(&rule.id), State::Retrying);
        assert_eq!(manager.state(&other.id), State::On);
        manager.poll(now + Duration::from_secs(1));
        assert_eq!(state.lock().unwrap().spawned, 2);
        manager.poll(now + Duration::from_secs(2));
        assert_eq!(state.lock().unwrap().spawned, 3);
        state
            .lock()
            .unwrap()
            .exited
            .insert(3, (255, "Connection timed out".into()));
        manager.poll(now + Duration::from_secs(3));
        manager.stop(&rule.id);
        manager.poll(now + Duration::from_secs(100));
        assert_eq!(state.lock().unwrap().spawned, 3);
        assert_eq!(manager.state(&rule.id), State::Off);
        assert_eq!(manager.state(&other.id), State::On);
    }
    #[test]
    fn auth_and_key_errors_do_not_loop() {
        for message in [
            "Permission denied (publickey)",
            "Host key verification failed",
            "Address already in use",
        ] {
            let (mut manager, state, rule, now) = fixture();
            manager.start(&rule, now).unwrap();
            state
                .lock()
                .unwrap()
                .exited
                .insert(1, (255, message.into()));
            manager.poll(now);
            manager.poll(now + Duration::from_secs(999));
            assert_eq!(manager.state(&rule.id), State::Error);
            assert_eq!(state.lock().unwrap().spawned, 1);
        }
    }
    #[test]
    fn probe_failure_does_not_kill_healthy_process_or_claim_initial_success() {
        let (mut manager, state, rule, now) = fixture();
        manager.start(&rule, now).unwrap();
        state.lock().unwrap().probe_error = true;
        manager.poll(now + Duration::from_secs(30));
        assert_eq!(manager.state(&rule.id), State::Connecting);
        assert!(state.lock().unwrap().stopped.is_empty());
        assert!(manager.details(&rule.id, now).contains("Cannot verify"));
        state.lock().unwrap().probe_error = false;
        state.lock().unwrap().owned.insert((1, 18000));
        manager.poll(now + Duration::from_secs(31));
        assert_eq!(manager.state(&rule.id), State::On);
    }
    #[test]
    fn conflicting_requested_ports_fail_locally_and_edit_restarts_once() {
        let (mut manager, state, rule, now) = fixture();
        manager.start(&rule, now).unwrap();
        let duplicate = Forward::new(18000, 9999, "Other").unwrap();
        manager.start(&duplicate, now).unwrap();
        assert_eq!(manager.state(&duplicate.id), State::Error);
        assert_eq!(state.lock().unwrap().spawned, 1);
        let mut edited = rule.clone();
        edited.remote_port = 8001;
        manager.restart(&edited, now).unwrap();
        assert_eq!(state.lock().unwrap().spawned, 2);
        assert_eq!(state.lock().unwrap().stopped, [1]);
    }
}
