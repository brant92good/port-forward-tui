//! Owned SSH processes and OS listener ownership. No connection probes to services.
use crate::store::{Forward, Settings};
#[cfg(any(windows, target_os = "macos"))]
use anyhow::ensure;
use anyhow::{Context, Result};
use std::{
    collections::{HashSet, VecDeque},
    io::Read,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(windows)]
#[path = "process/windows.rs"]
mod platform;
#[cfg(unix)]
#[path = "process/unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "process/windows_daemon.rs"]
mod windows_daemon;
#[cfg(windows)]
pub use windows_daemon::DaemonChild;
#[cfg(unix)]
pub type DaemonChild = Child;
pub type HiddenChild = DaemonChild;

pub type Listeners = HashSet<(u32, u16)>;

/// Run once at CLI startup, before background workers or child processes exist.
pub fn protect_incoming_stdio() -> Result<()> {
    #[cfg(windows)]
    platform::protect_incoming_stdio()?;
    Ok(())
}

pub trait TunnelProcess: Send {
    fn id(&self) -> u32;
    fn exited(&mut self) -> Result<Option<i32>>;
    fn stop(&mut self);
    fn details(&self) -> String;
}

pub trait Backend: Send {
    fn start(&mut self, settings: &Settings, rule: &Forward) -> Result<Box<dyn TunnelProcess>>;
    fn listeners(&mut self, pids: &[u32]) -> Result<Listeners>;
}

pub fn ssh_arguments(settings: &Settings, rule: &Forward) -> Result<Vec<String>> {
    rule.validate()?;
    let mut args = Vec::new();
    if let Some(port) = settings.ssh_port {
        args.extend(["-p".into(), port.to_string()]);
    }
    if let Some(config) = &settings.ssh_config {
        args.extend(["-F".into(), config.clone()]);
    }
    args.extend(["-N".into(), "-T".into()]);
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ExitOnForwardFailure=yes",
        "ConnectTimeout=10",
        "ConnectionAttempts=1",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ControlMaster=no",
        "ControlPath=none",
        "ForkAfterAuthentication=no",
        "LogLevel=ERROR",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    if rule.is_socks() {
        args.extend(["-D".into(), format!("127.0.0.1:{}", rule.local_port)]);
    } else {
        let remote = rule
            .remote_port
            .context("A port forward needs a remote port.")?;
        args.extend([
            "-L".into(),
            format!("127.0.0.1:{}:127.0.0.1:{remote}", rule.local_port),
        ]);
    }
    args.push(settings.host.clone());
    Ok(args)
}

pub fn ssh_executable() -> Result<PathBuf> {
    which::which(if cfg!(windows) { "ssh.exe" } else { "ssh" })
        .context("OpenSSH was not found. Install the OpenSSH client and try again.")
}

/// Validate availability without sending traffic to a possibly unrelated service.
fn check_local_port(port: u16) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    return check_local_port_once(port);
    #[cfg(target_os = "macos")]
    {
        // Concurrent socket inspection can briefly retain a closing macOS
        // socket. Both reuse-enabled bind and a normal listener then return
        // EADDRINUSE. Give only that idempotent availability check a bounded
        // grace period; never retry an SSH command or a controller mutation.
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            let result = check_local_port_once(port);
            if result.as_ref().err().is_some_and(|error| {
                error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.raw_os_error() == Some(libc::EADDRINUSE))
            }) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if !remaining.is_zero() {
                    thread::sleep(remaining.min(Duration::from_millis(5)));
                    continue;
                }
            }
            return result;
        }
    }
}

fn check_local_port_once(port: u16) -> Result<()> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    #[cfg(windows)]
    platform::exclusive(&socket)?;
    // OpenSSH and std's Unix listener can rebind after accepted connections
    // enter TIME_WAIT. Match that policy for the preflight; a live listener
    // still rejects the bind. Windows keeps exclusive ownership above.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())
        .with_context(|| {
            format!(
                "Local port {port} is already in use or unavailable. Choose another local port."
            )
        })?;
    Ok(())
}

pub struct NativeBackend {
    executable: PathBuf,
}

impl NativeBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            executable: ssh_executable()?,
        })
    }
    /// Explicit injection for isolated process integration tests, never a shell command.
    pub fn with_executable(executable: PathBuf) -> Self {
        Self { executable }
    }
}

impl Backend for NativeBackend {
    fn start(&mut self, settings: &Settings, rule: &Forward) -> Result<Box<dyn TunnelProcess>> {
        settings.validate()?;
        crate::store::host(&settings.host)?;
        rule.validate()?;
        check_local_port(rule.local_port)?;
        Ok(Box::new(OwnedSsh::spawn(
            &self.executable,
            &ssh_arguments(settings, rule)?,
        )?))
    }
    fn listeners(&mut self, pids: &[u32]) -> Result<Listeners> {
        platform::listeners(pids)
    }
}

struct OwnedSsh {
    child: Child,
    group: platform::Group,
    errors: Arc<Mutex<VecDeque<u8>>>,
    reader: Option<JoinHandle<()>>,
    cancel_reader: Arc<AtomicBool>,
}

impl OwnedSsh {
    fn spawn(executable: &Path, args: &[String]) -> Result<Self> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        platform::configure_ssh(&mut command);
        let mut child = command
            .spawn()
            .context("Could not start the OpenSSH client")?;
        // On Windows the child is suspended until the job owns it. A ProxyCommand
        // cannot escape in the interval between spawn and job assignment.
        let mut group = match platform::Group::attach(&mut child) {
            Ok(group) => group,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let Some(mut stderr) = child.stderr.take() else {
            group.stop(&mut child);
            anyhow::bail!("OpenSSH error stream was not created");
        };
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        let output = Arc::clone(&errors);
        let cancel_reader = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel_reader);
        let reader = thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            while !cancelled.load(Ordering::Relaxed) {
                let ready = match platform::readable(&stderr) {
                    Ok(ready) => ready,
                    Err(_) => break,
                };
                if ready == 0 {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                let size = ready.min(chunk.len());
                let Ok(count) = stderr.read(&mut chunk[..size]) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                if let Ok(mut bytes) = output.lock() {
                    bytes.extend(&chunk[..count]);
                    let remove = bytes.len().saturating_sub(6000);
                    bytes.drain(..remove);
                }
            }
        });
        Ok(Self {
            child,
            group,
            errors,
            reader: Some(reader),
            cancel_reader,
        })
    }
}

impl TunnelProcess for OwnedSsh {
    fn id(&self) -> u32 {
        self.child.id()
    }
    fn exited(&mut self) -> Result<Option<i32>> {
        #[cfg(unix)]
        return Ok(platform::exited(&self.child)?);
        #[cfg(windows)]
        Ok(self
            .child
            .try_wait()?
            .map(|status| status.code().unwrap_or(-1)))
    }
    fn stop(&mut self) {
        self.group.stop(&mut self.child);
        if let Some(reader) = self.reader.take() {
            let deadline = Instant::now() + Duration::from_millis(100);
            while !reader.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            self.cancel_reader.store(true, Ordering::Relaxed);
            let _ = reader.join();
        }
    }
    fn details(&self) -> String {
        self.errors
            .lock()
            .map(|bytes| {
                String::from_utf8_lossy(&bytes.iter().copied().collect::<Vec<_>>())
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    }
}
impl Drop for OwnedSsh {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start a controller with only its null input and log output handles.
pub fn spawn_daemon(
    executable: &Path,
    directory: &Path,
    log: std::fs::File,
) -> Result<DaemonChild> {
    #[cfg(windows)]
    return windows_daemon::spawn(executable, directory, log);
    #[cfg(unix)]
    {
        let mut command = Command::new(executable);
        command
            .args(["--serve", "--data-dir"])
            .arg(directory)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        platform::configure_daemon(&mut command);
        Ok(command.spawn()?)
    }
}

/// Dispatch a status-only helper with null streams and inherited environment/cwd.
/// Windows excludes all other handles and keeps the caller's job membership.
pub fn spawn_hidden(executable: &Path, args: &[std::ffi::OsString]) -> Result<HiddenChild> {
    #[cfg(windows)]
    return windows_daemon::hidden(executable, args);
    #[cfg(unix)]
    Ok(Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}

/// Read-only ownership probe, also used by integration checks.
pub fn owned_listeners(pids: &[u32]) -> Result<Listeners> {
    platform::listeners(pids)
}

/// Bounded helper execution for platform probes; no shell and no unbounded output.
#[cfg(target_os = "macos")]
pub(crate) fn capture_bounded(command: &mut Command, timeout: Duration) -> Result<(i32, Vec<u8>)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().context("No helper output pipe")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            anyhow::bail!("The local listener probe timed out.");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Listener probe reader stopped"))??;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Listener probe output exceeded its limit."
    );
    Ok((status.code().unwrap_or(-1), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_transport_is_noninteractive_and_port_values_are_data() {
        let settings = Settings {
            host: "workbox".into(),
            ssh_port: Some(2222),
            ssh_config: Some("C:/path with spaces/config".into()),
            ..Settings::default()
        };
        let rule = Forward::new(18000, 8000, "Web").unwrap();
        let args = ssh_arguments(&settings, &rule).unwrap();
        assert_eq!(
            &args[..4],
            ["-p", "2222", "-F", "C:/path with spaces/config"]
        );
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ExitOnForwardFailure=yes",
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            assert!(args.contains(&option.into()));
        }
        assert_eq!(
            &args[args.len() - 3..],
            ["-L", "127.0.0.1:18000:127.0.0.1:8000", "workbox"]
        );
    }
    #[test]
    fn listener_probe_identifies_owner_and_does_not_send_service_traffic() {
        use std::net::TcpListener;
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        socket.set_nonblocking(true).unwrap();
        let owned = owned_listeners(&[std::process::id()]).unwrap();
        assert!(owned.contains(&(std::process::id(), port)));
        assert!(socket.accept().is_err());
        assert!(check_local_port(port).is_err());
        drop(socket);
        assert!(check_local_port(port).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn unix_preflight_allows_recovery_after_traffic_but_rejects_live_listener() {
        use std::{
            io::Write,
            net::{Shutdown, TcpListener, TcpStream},
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        #[cfg(target_os = "macos")]
        let _observer = RebindObserver::start();
        let diagnostic = std::env::var_os("PORTS_REBIND_DIAGNOSTIC").is_some();
        let listener_reuse = socket2::SockRef::from(&listener).reuse_address();
        assert!(check_local_port(address.port()).is_err());
        let mut client = TcpStream::connect(address).unwrap();
        let peer = client.local_addr().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (mut service, _) = listener.accept().unwrap();
        service.write_all(b"served").unwrap();
        service
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // Complete both FIN directions before asserting TIME_WAIT. Merely
        // dropping the peer can leave an asynchronously closing connection
        // briefly in FIN_WAIT_2 on macOS, which is not the state under test.
        service.shutdown(Shutdown::Write).unwrap();
        let mut received = Vec::new();
        client.read_to_end(&mut received).unwrap();
        assert_eq!(received, b"served");
        client.shutdown(Shutdown::Write).unwrap();
        service.read_to_end(&mut Vec::new()).unwrap();
        drop(service);
        drop(client);
        let closing = Instant::now();
        drop(listener);
        let without_reuse = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        assert_eq!(
            without_reuse.bind(&address.into()).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        // Retain the raw first attempt so diagnostics can show the original
        // transient still exists while the product handles it within its bound.
        let initial = check_local_port_once(address.port());
        let initial_elapsed = closing.elapsed();
        let bounded_started = Instant::now();
        let bounded = check_local_port(address.port());
        let bounded_elapsed = bounded_started.elapsed();
        if diagnostic || initial.is_err() {
            eprintln!(
                "REBIND bounded_us={} result={bounded:?}",
                bounded_elapsed.as_micros()
            );
            eprintln!(
                "REBIND initial_us={} pid={} local={address} peer={peer} listener_reuse={listener_reuse:?} failed_probe_local={:?} result={initial:?}",
                initial_elapsed.as_micros(),
                std::process::id(),
                without_reuse.local_addr(),
            );
            if let Err(error) = &initial {
                eprintln!("REBIND initial_chain={error:#}");
                for cause in error.chain() {
                    if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                        eprintln!(
                            "REBIND initial_errno={:?} kind={:?}",
                            io.raw_os_error(),
                            io.kind()
                        );
                    }
                }
            }
            // Diagnostic probes may affect cleanup. They run after the product
            // check; retain and report its result without retrying the test.
            rebind_pair(address, closing, "failed-nonreuse-socket-still-open");
        }
        drop(without_reuse);
        if diagnostic || initial.is_err() {
            for target in [0, 1, 10, 50, 250] {
                let target = Duration::from_millis(target);
                if let Some(delay) = target.checked_sub(closing.elapsed()) {
                    thread::sleep(delay);
                }
                rebind_pair(address, closing, "failed-nonreuse-socket-closed");
            }
            #[cfg(target_os = "macos")]
            rebind_socket_table(address.port());
        }
        assert!(bounded.is_ok(), "Bounded preflight failed: {bounded:?}");
        let reopened = TcpListener::bind(address).unwrap();
        assert!(check_local_port(address.port()).is_err());
        drop(reopened);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_live_port_remains_rejected_after_bounded_grace_period() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let started = Instant::now();
        let result = check_local_port(listener.local_addr().unwrap().port());
        let elapsed = started.elapsed();
        assert!(result.is_err(), "A live listener must never pass preflight");
        assert!(elapsed >= Duration::from_millis(250));
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::EADDRINUSE)
        );
    }

    #[cfg(unix)]
    fn rebind_pair(address: std::net::SocketAddr, started: Instant, phase: &str) {
        let raw = (|| -> std::io::Result<()> {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            socket.set_reuse_address(true)?;
            socket.bind(&address.into())
        })();
        eprintln!(
            "REBIND elapsed_us={} phase={phase} raw_reuse={raw:?} raw_errno={:?}",
            started.elapsed().as_micros(),
            raw.as_ref().err().and_then(std::io::Error::raw_os_error),
        );
        // Drop this socket before another probe; it is never an extra listener.
        let standard = std::net::TcpListener::bind(address).map(drop);
        eprintln!(
            "REBIND elapsed_us={} phase={phase} std_listener={standard:?} std_errno={:?}",
            started.elapsed().as_micros(),
            standard
                .as_ref()
                .err()
                .and_then(std::io::Error::raw_os_error),
        );
    }

    #[cfg(target_os = "macos")]
    fn rebind_socket_table(port: u16) {
        // Query only after timed bind probes: observing sockets can affect
        // their lifetime. Emit only rows containing the fixture's exact port.
        let result = capture_bounded(
            Command::new("/usr/sbin/netstat").args(["-anv", "-p", "tcp"]),
            Duration::from_secs(2),
        );
        match result {
            Ok((status, bytes)) => {
                eprintln!("REBIND netstat_status={status} (snapshot after timed probes)");
                let suffix = format!(".{port}");
                for line in String::from_utf8_lossy(&bytes)
                    .lines()
                    .filter(|line| line.split_whitespace().any(|part| part.ends_with(&suffix)))
                    .take(20)
                {
                    eprintln!("REBIND netstat={line}");
                }
            }
            Err(error) => eprintln!("REBIND netstat_error={error:#}"),
        }
    }

    #[cfg(target_os = "macos")]
    struct RebindObserver {
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<usize>>,
    }
    #[cfg(target_os = "macos")]
    impl RebindObserver {
        fn start() -> Option<Self> {
            std::env::var_os("PORTS_REBIND_OWNERSHIP_PROBE")?;
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = Arc::clone(&stop);
            let (ready, receiver) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                let mut calls = 0;
                while !stopping.load(Ordering::Relaxed) {
                    let result = owned_listeners(&[std::process::id()]);
                    calls += 1;
                    if calls == 1 {
                        let _ = ready.send(());
                    }
                    if let Err(error) = result {
                        eprintln!("REBIND ownership_probe_error={error:#}");
                    }
                }
                calls
            });
            let ready = receiver.recv_timeout(Duration::from_secs(3));
            eprintln!("REBIND ownership_probe_ready={ready:?}");
            Some(Self {
                stop,
                worker: Some(worker),
            })
        }
    }
    #[cfg(target_os = "macos")]
    impl Drop for RebindObserver {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                eprintln!("REBIND ownership_probe_calls={:?}", worker.join());
            }
        }
    }
}
