use crate::store::{self, Lock, Store, absolute};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub target: String,
    #[serde(skip)]
    pub directory: PathBuf,
    pub ssh_port: Option<u16>,
    pub ssh_config: Option<String>,
}
// Match Python json.dumps(..., ensure_ascii=True), including spaces and surrogate escapes.
fn ascii_json(value: &str) -> String {
    let json = serde_json::to_string(value).expect("String serialization is infallible");
    let mut output = String::new();
    for c in json.chars() {
        if c.is_ascii() {
            output.push(c);
        } else {
            let mut buffer = [0; 2];
            for unit in c.encode_utf16(&mut buffer) {
                output.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    output
}
pub fn machine_id(target: &str, port: Option<u16>, config: Option<&str>) -> Result<String> {
    store::host(target)?;
    let payload = format!(
        "[{}, {}, {}]",
        ascii_json(target),
        port.map(|p| p.to_string()).unwrap_or_else(|| "null".into()),
        config.map(ascii_json).unwrap_or_else(|| "null".into())
    );
    Ok(format!("{:x}", Sha256::digest(payload.as_bytes()))[..32].into())
}
#[derive(Debug, Clone)]
pub struct Catalog {
    pub directory: PathBuf,
}
impl Catalog {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            directory: absolute(directory)?,
        })
    }
    pub fn list(&self) -> Result<Vec<Machine>> {
        let mut folders = Vec::new();
        if self.directory.join("forwards.json").is_file() {
            folders.push(self.directory.clone());
        }
        let root = self.directory.join("machines");
        if root.is_dir() {
            let mut nested = fs::read_dir(root)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.join("forwards.json").is_file())
                .collect::<Vec<_>>();
            nested.sort();
            folders.extend(nested);
        }
        let mut result = Vec::new();
        for directory in folders {
            let store = Store::load(&directory)?;
            let settings = store.settings;
            if settings.host.is_empty() {
                continue;
            }
            let id = machine_id(
                &settings.host,
                settings.ssh_port,
                settings.ssh_config.as_deref(),
            )?;
            ensure!(
                directory == self.directory
                    || directory.file_name().is_some_and(|s| s == id.as_str()),
                "Machine destination changed. Restore the settings and add a separate machine instead."
            );
            result.push(Machine {
                id,
                name: if settings.machine_name.is_empty() {
                    settings.host.clone()
                } else {
                    settings.machine_name
                },
                target: settings.host,
                directory,
                ssh_port: settings.ssh_port,
                ssh_config: settings.ssh_config,
            });
        }
        Ok(result)
    }
    pub fn get(&self, value: &str) -> Result<Machine> {
        let machines = self.list()?;
        if let Some(machine) = machines.iter().find(|m| m.id == value) {
            return Ok(machine.clone());
        }
        let matches = machines
            .into_iter()
            .filter(|m| m.name == value || m.target == value)
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Select a machine by unique ID or exact name. Run machines list."
        );
        Ok(matches[0].clone())
    }
    pub fn add(
        &self,
        target: &str,
        name: &str,
        ssh_port: Option<u16>,
        config: Option<&Path>,
    ) -> Result<Machine> {
        let target = store::host(target.trim())?;
        let name = store::name(name, true)?;
        ensure!(ssh_port != Some(0), "Invalid SSH port.");
        let config = config.map(absolute).transpose()?;
        if let Some(config) = &config {
            ensure!(config.is_file(), "SSH configuration file does not exist.");
        }
        let config = config.map(|p| p.to_string_lossy().into_owned());
        let id = machine_id(&target, ssh_port, config.as_deref())?;
        crate::channel::prepare(&self.directory)?;
        let _lock = Lock::acquire(&self.directory, "catalog.lock", Duration::from_secs(3))?;
        if let Some(machine) = self.list()?.into_iter().find(|m| m.id == id) {
            return Ok(machine);
        }
        let directory = self.directory.join("machines").join(&id);
        let mut store = Store::load(&directory)?;
        store.settings.host = target;
        store.settings.ssh_port = ssh_port;
        store.settings.ssh_config = config;
        store.settings.machine_name = name;
        store.initialize()?;
        self.get(&id)
    }
}
pub fn default_ssh_config() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh/config")
}
fn home_path(value: &str) -> PathBuf {
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        value.into()
    }
}
fn words(line: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in line.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else if c == '#' {
            break;
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace()
            || (c == '=' && (result.is_empty() || result.len() == 1 && !started))
        {
            if started {
                result.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    ensure!(quote.is_none(), "Unclosed quote in SSH config.");
    if started {
        result.push(word);
    }
    Ok(result)
}
pub fn ssh_aliases(config: Option<&Path>) -> Result<Vec<String>> {
    fn read(
        path: &Path,
        depth: usize,
        visited: &mut BTreeSet<PathBuf>,
        aliases: &mut BTreeSet<String>,
    ) -> Result<()> {
        ensure!(
            depth <= 16 && visited.len() < 256,
            "Too many nested SSH Include files."
        );
        let path = absolute(path)?;
        if !visited.insert(path.clone()) {
            return Ok(());
        }
        let raw = fs::read(path)?;
        ensure!(raw.len() <= 2 * 1024 * 1024, "SSH config is too large.");
        let text = std::str::from_utf8(raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&raw))?;
        for line in text.lines() {
            let tokens = words(line)?;
            let Some(key) = tokens.first() else {
                continue;
            };
            if key.eq_ignore_ascii_case("host") {
                for value in &tokens[1..] {
                    if store::host(value).is_ok() {
                        aliases.insert(value.clone());
                    }
                }
            } else if key.eq_ignore_ascii_case("include") {
                for value in &tokens[1..] {
                    if value.contains('%') || value.contains("${") {
                        continue;
                    }
                    let candidate = home_path(value);
                    let candidate = if candidate.is_absolute() {
                        candidate
                    } else {
                        dirs::home_dir()
                            .context("No home directory")?
                            .join(".ssh")
                            .join(candidate)
                    };
                    let mut paths = glob::glob(&candidate.to_string_lossy())?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    paths.sort();
                    for child in paths {
                        if child.is_file() {
                            read(&child, depth + 1, visited, aliases)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    let path = config
        .map(Path::to_path_buf)
        .unwrap_or_else(default_ssh_config);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut aliases = BTreeSet::new();
    read(&path, 0, &mut BTreeSet::new(), &mut aliases)?;
    let mut aliases = aliases.into_iter().collect::<Vec<_>>();
    aliases.sort_by_key(|s| s.to_lowercase());
    Ok(aliases)
}
pub fn import(
    catalog: &Catalog,
    config: Option<&Path>,
    selected: Option<&[String]>,
) -> Result<Vec<Machine>> {
    let available = ssh_aliases(config)?;
    let selected = selected.unwrap_or(&available);
    ensure!(
        selected.iter().all(|s| available.contains(s)),
        "Choose aliases from the import preview."
    );
    let source = config.map(absolute).transpose()?;
    let source = source.filter(|p| Some(p) != absolute(&default_ssh_config()).ok().as_ref());
    selected
        .iter()
        .map(|alias| catalog.add(alias, "", None, source.as_deref()))
        .collect()
}
