use super::*;
use std::os::unix::process::CommandExt;
pub fn readable(pipe: &std::process::ChildStderr) -> std::io::Result<usize> {
    use std::os::fd::AsRawFd;
    let mut descriptor = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if ready < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // A ready/HUP pipe can be read without blocking. This reader is its only consumer.
    Ok(if ready == 0 { 0 } else { 4096 })
}

pub fn configure_ssh(command: &mut Command) {
    command.process_group(0);
}
pub fn configure_daemon(command: &mut Command) {
    // setsid is async-signal-safe. The child has only explicitly supplied null/log
    // streams and will not belong to the terminal's session/process group.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

pub struct Group {
    pid: Option<i32>,
}
impl Group {
    pub fn attach(child: &mut Child) -> Result<Self> {
        Ok(Self {
            pid: Some(i32::try_from(child.id())?),
        })
    }
    pub fn stop(&mut self, child: &mut Child) {
        let Some(pid) = self.pid.take() else {
            return;
        };
        // This child was created as its own process group; never signal a name,
        // a global process list, or a group inherited from the user's terminal.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait();
    }
}

#[cfg(target_os = "linux")]
pub fn listeners(pids: &[u32]) -> Result<Listeners> {
    use std::{collections::HashMap, fs};
    if pids.is_empty() {
        return Ok(HashSet::new());
    }
    let table =
        fs::read_to_string("/proc/net/tcp").context("Cannot read Linux TCP listener ownership")?;
    let mut sockets = HashMap::new();
    for line in table.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() <= 9 || fields[3] != "0A" {
            continue;
        }
        if let Some((_, port)) = fields[1].split_once(':') {
            if let Ok(port) = u16::from_str_radix(port, 16) {
                sockets.insert(fields[9].to_owned(), port);
            }
        }
    }
    let mut result = HashSet::new();
    for &pid in pids {
        let path = PathBuf::from(format!("/proc/{pid}/fd"));
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("Cannot inspect the SSH process sockets"),
        };
        for entry in entries.flatten() {
            if let Ok(target) = fs::read_link(entry.path()) {
                let target = target.to_string_lossy();
                if let Some(inode) = target
                    .strip_prefix("socket:[")
                    .and_then(|text| text.strip_suffix(']'))
                {
                    if let Some(&port) = sockets.get(inode) {
                        result.insert((pid, port));
                    }
                }
            }
        }
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
pub fn listeners(pids: &[u32]) -> Result<Listeners> {
    if pids.is_empty() {
        return Ok(HashSet::new());
    }
    let wanted: HashSet<_> = pids.iter().copied().collect();
    let executable = Path::new("/usr/sbin/lsof");
    ensure!(
        executable.is_file(),
        "macOS listener checks need the system /usr/sbin/lsof tool."
    );
    let ids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // -a intersects PID/TCP/LISTEN filters; -Fpn returns machine-readable owner
    // and numeric endpoint fields. No DNS lookup, service probing or shell.
    let (status, output) = capture_bounded(
        Command::new(executable).args(["-nP", "-a", "-p", &ids, "-iTCP", "-sTCP:LISTEN", "-Fpn"]),
        Duration::from_secs(2),
    )?;
    ensure!(
        status == 0 || status == 1 && output.is_empty(),
        "macOS listener ownership probe failed ({status})"
    );
    let mut result = HashSet::new();
    let mut owner = None;
    for line in String::from_utf8_lossy(&output).lines() {
        if let Some(pid) = line.strip_prefix('p') {
            owner = pid.parse::<u32>().ok().filter(|pid| wanted.contains(pid));
        } else if let (Some(pid), Some(endpoint)) = (owner, line.strip_prefix('n')) {
            if let Some((_, port)) = endpoint.rsplit_once(':') {
                if let Ok(port) = port.parse::<u16>() {
                    result.insert((pid, port));
                }
            }
        }
    }
    Ok(result)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn listeners(_pids: &[u32]) -> Result<Listeners> {
    anyhow::bail!("This operating system has no qualified listener ownership backend.")
}
