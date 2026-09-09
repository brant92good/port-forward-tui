use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub fn default_directory() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PortForwardTUI")
}
pub fn host(value: &str) -> Result<String> {
    ensure!(
        value
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.@-".contains(&c)),
        "Use an SSH config alias or user@hostname, without spaces or options."
    );
    Ok(value.into())
}
pub fn port(value: &str) -> Result<u16> {
    ensure!(
        !value.is_empty() && value.len() <= 5 && value.bytes().all(|c| c.is_ascii_digit()),
        "Use a port number from 1 to 65535."
    );
    let number: u16 = value
        .parse()
        .context("Ports must be between 1 and 65535.")?;
    ensure!(number > 0, "Ports must be between 1 and 65535.");
    Ok(number)
}
pub fn quick_ports(value: &str) -> Result<(u16, u16)> {
    let parts: Vec<_> = value.trim().split(':').collect();
    match parts.as_slice() {
        [one] => {
            let number = port(one)?;
            Ok((number, number))
        }
        [local, remote] => Ok((port(local.trim())?, port(remote.trim())?)),
        _ => anyhow::bail!("Type PORT or LOCAL:REMOTE, for example 8000 or 18000:8000."),
    }
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn name(value: &str, optional: bool) -> Result<String> {
    let text = value.trim();
    ensure!(
        (optional || !text.is_empty())
            && text.chars().count() <= 80
            && !text.chars().any(char::is_control),
        "Use a name of 1–80 readable characters."
    );
    Ok(text.into())
}
fn deserialize_port<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u16, D::Error> {
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    let text = match value {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) if n.is_u64() => n.to_string(),
        _ => return Err(D::Error::custom("Invalid port number")),
    };
    port(&text).map_err(D::Error::custom)
}
fn optional_port<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u16>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    value
        .map(|value| deserialize_port(value).map_err(serde::de::Error::custom))
        .transpose()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_port")]
    pub local_port: u16,
    #[serde(deserialize_with = "deserialize_port")]
    pub remote_port: u16,
}
impl Forward {
    pub fn new(local_port: u16, remote_port: u16, label: &str) -> Result<Self> {
        let name = name(label, true)?;
        let rule = Self {
            id: new_id(),
            name: if name.is_empty() {
                format!("Port {remote_port}")
            } else {
                name
            },
            local_port,
            remote_port,
        };
        rule.validate()?;
        Ok(rule)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_id(&self.id), "Invalid favorite ID.");
        ensure!(
            self.name.chars().count() <= 80,
            "Favorite name exceeds 80 characters."
        );
        ensure!(
            self.local_port > 0 && self.remote_port > 0,
            "Ports must be between 1 and 65535."
        );
        Ok(())
    }
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub version: u8,
    pub host: String,
    #[serde(default = "yes")]
    pub keep_alive: bool,
    pub forwards: Vec<Forward>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_port"
    )]
    pub ssh_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_config: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine_name: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            host: String::new(),
            keep_alive: true,
            forwards: Vec::new(),
            ssh_port: None,
            ssh_config: None,
            machine_name: String::new(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported favorites file version.");
        if !self.host.is_empty() {
            host(&self.host)?;
        }
        ensure!(self.ssh_port != Some(0), "Invalid SSH port.");
        ensure!(
            self.ssh_config.as_ref().is_none_or(|s| !s.is_empty()),
            "Invalid SSH configuration path."
        );
        ensure!(
            self.machine_name.chars().count() <= 80,
            "Invalid machine name."
        );
        let mut ids = HashSet::new();
        for rule in &self.forwards {
            rule.validate()?;
            ensure!(ids.insert(&rule.id), "Duplicate saved forward IDs.");
        }
        Ok(())
    }
    pub fn same_destination(&self, other: &Self) -> bool {
        self.host == other.host
            && self.ssh_port == other.ssh_port
            && self.ssh_config == other.ssh_config
    }
}
#[derive(Debug, Clone)]
pub struct Store {
    pub directory: PathBuf,
    pub settings: Settings,
}
impl Store {
    /// Read-only: an absent file does not create defaults or start a controller.
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("forwards.json");
        let settings = if path.exists() {
            let raw = fs::read(path)?;
            ensure!(
                raw.len() <= 4 * 1024 * 1024,
                "Favorites file exceeds the size limit."
            );
            let settings: Settings = serde_json::from_slice(&raw)
                .context("Invalid favorites JSON. Keep a backup before repairing it.")?;
            settings.validate()?;
            settings
        } else {
            Settings::default()
        };
        Ok(Self {
            directory: directory.to_path_buf(),
            settings,
        })
    }
    pub fn path(&self) -> PathBuf {
        self.directory.join("forwards.json")
    }
    pub fn save(&mut self, forwards: Vec<Forward>) -> Result<()> {
        let mut settings = self.settings.clone();
        settings.forwards = forwards;
        settings.validate()?;
        write_json(&self.path(), &settings)?;
        self.settings = settings;
        Ok(())
    }
    pub fn initialize(&mut self) -> Result<()> {
        ensure!(!self.path().exists(), "A favorites file already exists.");
        let rules = [
            (3000, "Web app"),
            (5173, "Vite"),
            (8000, "API / dev server"),
            (8080, "Web server"),
            (8888, "Jupyter"),
            (6006, "TensorBoard"),
        ]
        .into_iter()
        .map(|(p, n)| Forward::new(p, p, n))
        .collect::<Result<Vec<_>>>()?;
        self.save(rules)
    }
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut raw = serde_json::to_vec_pretty(value)?;
    raw.push(b'\n');
    let parent = path.parent().context("File needs a parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&raw)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
impl Lock {
    pub fn acquire(directory: &Path, filename: &str, wait: Duration) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(filename))?;
        if file.metadata()?.len() == 0 {
            file.write_all(b"0")?;
            file.flush()?;
        }
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) => {
                    if Instant::now() >= deadline {
                        return Err(error)
                            .with_context(|| format!("Another process holds {filename}."));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(part.as_os_str()),
        }
        if matches!(part, std::path::Component::Normal(_)) && result.exists() {
            result = fs::canonicalize(&result)?;
            #[cfg(windows)]
            {
                let text = result.to_string_lossy();
                result = if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
                    PathBuf::from(format!("\\\\{unc}"))
                } else {
                    PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text))
                };
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn missing_parent_segments_resolve_without_creating_directories() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            absolute(&temp.path().join("missing/../data")).unwrap(),
            absolute(&temp.path().join("data")).unwrap()
        );
        assert!(!temp.path().join("missing").exists());
        assert!(!temp.path().join("data").exists());
    }
}
