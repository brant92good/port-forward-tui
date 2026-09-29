//! Explicit, optional audio transport. No startup or retry integration with forwards.
use crate::{machines::Machine, store};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub mod panel;
#[cfg(windows)]
mod windows;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Opt this machine into audio. Does not connect or open a microphone.
    Configure {
        #[arg(long)]
        microphone: String,
        #[arg(long)]
        remote_script: String,
        #[arg(long)]
        expected_host: String,
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        ffplay: PathBuf,
    },
    /// Inspect local state only; never connects or records.
    Status,
    /// Explicitly start microphone forwarding and reply playback.
    Start,
    /// Stop owned audio processes and check remote default restoration.
    Stop,
    /// Disable audio after stopping it.
    Disable,
    #[command(hide = true)]
    Run {
        #[arg(long)]
        token: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub enabled: bool,
    pub microphone: String,
    pub remote_script: String,
    pub expected_host: String,
    pub ffmpeg: PathBuf,
    pub ffplay: PathBuf,
}
pub fn load(machine: &Machine) -> Result<Option<Config>> {
    let path = machine.directory.join("audio.json");
    match fs::read(path) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 65536, "Audio configuration is too large");
            let config: Config = serde_json::from_slice(&bytes)?;
            config.validate()?;
            Ok(Some(config))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
impl Config {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported audio configuration version");
        for text in [&self.microphone, &self.remote_script, &self.expected_host] {
            ensure!(
                !text.is_empty() && text.len() < 4096 && !text.chars().any(char::is_control),
                "Invalid audio configuration text"
            );
        }
        ensure!(
            self.remote_script.starts_with('/'),
            "Remote script must be an absolute Linux path"
        );
        ensure!(
            self.ffmpeg.is_absolute() && self.ffplay.is_absolute(),
            "Audio executable paths must be absolute"
        );
        Ok(())
    }
}
/// Remote argv is parsed by the SSH login shell; quote the path, never interpolate raw text.
pub fn remote_command(script: &str, action: &str) -> String {
    format!("python3 '{}' {}", script.replace('\'', "'\"'\"'"), action)
}
pub fn ssh_args(machine: &Machine, config: &Config, action: &str) -> Result<Vec<String>> {
    store::host(&machine.target)?;
    let mut args = vec!["-T".into()];
    if let Some(port) = machine.ssh_port {
        args.extend(["-p".into(), port.to_string()]);
    }
    if let Some(path) = &machine.ssh_config {
        args.extend(["-F".into(), path.clone()]);
    }
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ConnectTimeout=10",
        "ConnectionAttempts=1",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ControlMaster=no",
        "ControlPath=none",
        "ForkAfterAuthentication=no",
        "ClearAllForwardings=yes",
        "RemoteCommand=none",
        "StdinNull=no",
        "RequestTTY=no",
        "LogLevel=ERROR",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    args.extend([
        machine.target.clone(),
        format!(
            "test \"$(hostname)\" = '{}' && exec {}",
            config.expected_host.replace('\'', "'\"'\"'"),
            remote_command(&config.remote_script, action)
        ),
    ]);
    Ok(args)
}
pub fn execute(machine: &Machine, root: &Path, action: &Action) -> Result<Value> {
    #[cfg(windows)]
    let _configuration_guard = if matches!(action, Action::Configure { .. } | Action::Disable) {
        Some(windows::command_guard(machine)?)
    } else {
        None
    };
    match action {
        Action::Configure {
            microphone,
            remote_script,
            expected_host,
            ffmpeg,
            ffplay,
        } => {
            ensure!(
                cfg!(windows),
                "Audio preview currently supports Windows capture/playback only"
            );
            ensure!(
                !running(machine)?,
                "Stop audio before changing its configuration"
            );
            let config = Config {
                version: 1,
                enabled: true,
                microphone: microphone.clone(),
                remote_script: remote_script.clone(),
                expected_host: expected_host.clone(),
                ffmpeg: ffmpeg.canonicalize().context("FFmpeg not found")?,
                ffplay: ffplay.canonicalize().context("FFplay not found")?,
            };
            config.validate()?;
            store::write_json(&machine.directory.join("audio.json"), &config)?;
            status(machine)
        }
        Action::Status => status(machine),
        Action::Disable => {
            stop(machine)?;
            if let Some(mut config) = load(machine)? {
                config.enabled = false;
                store::write_json(&machine.directory.join("audio.json"), &config)?;
            }
            status(machine)
        }
        Action::Start => start(machine, root),
        Action::Stop => stop(machine),
        Action::Run { token } => {
            #[cfg(windows)]
            {
                windows::run(machine, token)?;
                Ok(json!({"stopped":true}))
            }
            #[cfg(not(windows))]
            {
                let _ = token;
                anyhow::bail!("Audio preview currently supports Windows only")
            }
        }
    }
}
/// Read-only view status. Never starts capture or contacts the remote host.
pub fn indicator(machine: &Machine) -> String {
    match status(machine) {
        Err(_) => "Audio: check status [V]".into(),
        Ok(value) => {
            if value["supported"] != true {
                return "Audio: WINDOWS ONLY [V]".into();
            }
            let label = if value["running"] == true {
                value["last"]["phase"].as_str().unwrap_or("starting")
            } else if value["configuration_error"].is_string() {
                "configuration error"
            } else if value["enabled"] == true {
                "off"
            } else {
                "not configured"
            };
            format!("Audio: {} [V]", label.to_ascii_uppercase())
        }
    }
}
pub fn status(machine: &Machine) -> Result<Value> {
    let config = load(machine);
    let live = running(machine)?;
    let mut last: Value = fs::read(machine.directory.join("audio-state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(json!({"phase":"off"}));
    if !live
        && matches!(
            last["phase"].as_str(),
            Some("starting" | "streaming" | "stopping")
        )
    {
        last["phase"] = json!("interrupted");
        last["restoration"] =
            json!("unconfirmed: worker exited; Start checks the remote state again");
    }
    Ok(
        json!({"experimental":true, "supported":cfg!(windows), "enabled":config.as_ref().ok().and_then(|c|c.as_ref()).is_some_and(|c|c.enabled), "running":live, "microphone":config.as_ref().ok().and_then(|c|c.as_ref()).map(|c|&c.microphone), "playback":"Windows default output", "last":last, "configuration_error":config.err().map(|e|e.to_string()), "automatic_start":false, "automatic_reconnect":false}),
    )
}
pub fn running(machine: &Machine) -> Result<bool> {
    #[cfg(windows)]
    {
        windows::running(machine)
    }
    #[cfg(not(windows))]
    {
        let _ = machine;
        Ok(false)
    }
}
pub fn start(machine: &Machine, root: &Path) -> Result<Value> {
    #[cfg(windows)]
    {
        windows::start(machine, root)
    }
    #[cfg(not(windows))]
    {
        let _ = (machine, root);
        anyhow::bail!("Audio preview currently supports Windows only")
    }
}
pub fn stop(machine: &Machine) -> Result<Value> {
    #[cfg(windows)]
    {
        windows::stop(machine)
    }
    #[cfg(not(windows))]
    {
        status(machine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn machine(path: &Path) -> Machine {
        Machine {
            id: "test".into(),
            name: "test".into(),
            target: "workbox".into(),
            directory: path.into(),
            ssh_port: Some(2222),
            ssh_config: Some("custom config".into()),
        }
    }
    #[test]
    fn absent_config_is_disabled_and_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let value = status(&machine(temp.path())).unwrap();
        assert_eq!(value["enabled"], false);
        assert_eq!(value["running"], false);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn quote_remote_path_and_preserve_ssh_route() {
        let c = Config {
            version: 1,
            enabled: true,
            microphone: "physical mic".into(),
            remote_script: "/home/test/a'b/bridge.py".into(),
            expected_host: "test".into(),
            ffmpeg: PathBuf::new(),
            ffplay: PathBuf::new(),
        };
        let args = ssh_args(&machine(Path::new(".")), &c, "duplex").unwrap();
        assert!(args.windows(2).any(|v| v == ["-p", "2222"]));
        assert!(args.windows(2).any(|v| v == ["-F", "custom config"]));
        assert!(args.contains(&"ClearAllForwardings=yes".into()));
        assert!(!args.contains(&"-n".into()));
        assert!(
            args.last()
                .unwrap()
                .ends_with("python3 '/home/test/a'\"'\"'b/bridge.py' duplex")
        );
        assert!(
            args.last()
                .unwrap()
                .starts_with("test \"$(hostname)\" = 'test' && exec ")
        );
    }
}
