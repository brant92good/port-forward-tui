//! New-view preferences. Never part of the protocol-1 controller settings.
use crate::{
    machines::Machine,
    store::{self, Forward, Lock, Store},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    path::Path,
    time::Duration,
};

pub const FILE: &str = "forward-options.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    version: u8,
    pub open_automatically: BTreeSet<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            open_automatically: BTreeSet::new(),
        }
    }
}
impl Preferences {
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join(FILE);
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot read {}", path.display()));
            }
        };
        ensure!(raw.len() <= 4 * 1024 * 1024, "{FILE} is too large.");
        let value: Self =
            serde_json::from_slice(&raw).with_context(|| format!("Invalid {}", path.display()))?;
        ensure!(
            value.version == 1,
            "Unsupported {FILE} version {}.",
            value.version
        );
        ensure!(
            value
                .open_automatically
                .iter()
                .all(|id| store::valid_id(id)),
            "Invalid favorite ID in {FILE}."
        );
        Ok(value)
    }
    pub fn enabled(&self, id: &str) -> bool {
        self.open_automatically.contains(id)
    }
}

/// Merge under a distinct short-lived lock. Do not rewrite forwards.json or
/// take daemon.lock: old and native controllers may be using them right now.
pub fn set(directory: &Path, id: &str, enabled: bool) -> Result<()> {
    ensure!(store::valid_id(id), "Invalid favorite ID.");
    let _lock = Lock::acquire(directory, "forward-options.lock", Duration::from_secs(2))?;
    let store = Store::load(directory)?;
    ensure!(
        store.settings.forwards.iter().any(|rule| rule.id == id),
        "Favorite no longer exists. Refresh and choose a saved favorite."
    );
    let mut preferences = Preferences::load(directory)?;
    if preferences.enabled(id) == enabled {
        return Ok(());
    }
    if enabled {
        preferences.open_automatically.insert(id.into());
    } else {
        preferences.open_automatically.remove(id);
    }
    store::write_json(&directory.join(FILE), &preferences)
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub machine: Machine,
    pub rule: Forward,
    host: String,
    ssh_port: Option<u16>,
    ssh_config: Option<String>,
    keep_alive: bool,
}
impl Pending {
    /// Recheck at dispatch: a queued launch must not silently follow an edit.
    pub fn validate(&self) -> Result<()> {
        let current = Store::load(&self.machine.directory)?;
        ensure!(
            Preferences::load(&self.machine.directory)?.enabled(&self.rule.id),
            "Automatic opening was disabled; skipped {}.",
            self.rule.name
        );
        ensure!(
            current.settings.host == self.host
                && current.settings.ssh_port == self.ssh_port
                && current.settings.ssh_config == self.ssh_config
                && current.settings.keep_alive == self.keep_alive,
            "Machine settings changed; skipped automatic opening for {}.",
            self.machine.name
        );
        ensure!(
            current
                .settings
                .forwards
                .iter()
                .any(|rule| rule == &self.rule),
            "Favorite changed or was deleted; skipped automatic opening for {}.",
            self.rule.name
        );
        Ok(())
    }
    pub fn matches(&self, machine: &Machine, rule: &Forward) -> bool {
        self.machine.directory == machine.directory && self.rule.id == rule.id
    }
}

/// Called once, only after entering a genuinely new interactive view. Errors
/// disable this machine's automatic work, not its manual Stop controls.
pub fn collect(machines: &[Machine]) -> (VecDeque<Pending>, Vec<String>) {
    let mut pending = VecDeque::new();
    let mut warnings = Vec::new();
    for machine in machines {
        let result = (|| -> Result<()> {
            let store = Store::load(&machine.directory)?;
            let preferences = Preferences::load(&machine.directory)?;
            for rule in store
                .settings
                .forwards
                .iter()
                .filter(|rule| preferences.enabled(&rule.id))
            {
                pending.push_back(Pending {
                    machine: machine.clone(),
                    rule: rule.clone(),
                    host: store.settings.host.clone(),
                    ssh_port: store.settings.ssh_port,
                    ssh_config: store.settings.ssh_config.clone(),
                    keep_alive: store.settings.keep_alive,
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            warnings.push(format!(
                "{}: automatic opening skipped: {error:#}",
                machine.name
            ));
        }
    }
    (pending, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::Catalog;

    #[test]
    fn absent_options_are_readonly_and_concurrent_updates_preserve_schema_and_other_ids() {
        let temp = tempfile::tempdir().unwrap();
        let absent = temp.path().join("absent");
        assert!(
            Preferences::load(&absent)
                .unwrap()
                .open_automatically
                .is_empty()
        );
        assert!(!absent.exists());
        let machine = Catalog::new(temp.path())
            .unwrap()
            .add("fixture.invalid", "Fixture", None, None)
            .unwrap();
        let mut store = Store::load(&machine.directory).unwrap();
        let first = Forward::new(18001, 8001, "One").unwrap();
        let second = Forward::new(18002, 8002, "Two").unwrap();
        store.save(vec![first.clone(), second.clone()]).unwrap();
        let before = fs::read(machine.directory.join("forwards.json")).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| set(&machine.directory, &first.id, true).unwrap());
            scope.spawn(|| set(&machine.directory, &second.id, true).unwrap());
        });
        set(&machine.directory, &first.id, false).unwrap();
        let preferences = Preferences::load(&machine.directory).unwrap();
        assert!(!preferences.enabled(&first.id));
        assert!(preferences.enabled(&second.id));
        assert_eq!(
            fs::read(machine.directory.join("forwards.json")).unwrap(),
            before
        );
        assert!(!machine.directory.join("endpoint.json").exists());
        assert!(!machine.directory.join("daemon.lock").exists());
    }

    #[test]
    fn malformed_options_disable_only_their_machine_and_never_get_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::new(temp.path()).unwrap();
        let first = catalog.add("first.invalid", "First", None, None).unwrap();
        let second = catalog.add("second.invalid", "Second", None, None).unwrap();
        let rule = Store::load(&second.directory).unwrap().settings.forwards[0].clone();
        set(&second.directory, &rule.id, true).unwrap();
        for raw in [
            "broken",
            "{\"version\":2,\"open_automatically\":[]}",
            "{\"version\":1,\"open_automatically\":[],\"future\":true}",
        ] {
            fs::write(first.directory.join(FILE), raw).unwrap();
            let first_rule = Store::load(&first.directory).unwrap().settings.forwards[0].clone();
            assert!(set(&first.directory, &first_rule.id, true).is_err());
            assert_eq!(fs::read_to_string(first.directory.join(FILE)).unwrap(), raw);
            let (jobs, warnings) = collect(&[first.clone(), second.clone()]);
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].rule.id, rule.id);
            assert_eq!(warnings.len(), 1);
        }
    }

    #[test]
    fn queued_destinations_rules_and_enabled_choices_are_frozen() {
        let temp = tempfile::tempdir().unwrap();
        let machine = Catalog::new(temp.path())
            .unwrap()
            .add("fixture.invalid", "Fixture", None, None)
            .unwrap();
        let mut store = Store::load(&machine.directory).unwrap();
        let rule = store.settings.forwards[0].clone();
        set(&machine.directory, &rule.id, true).unwrap();
        let (queue, warnings) = collect(std::slice::from_ref(&machine));
        assert!(warnings.is_empty());
        let item = &queue[0];
        item.validate().unwrap();
        set(&machine.directory, &rule.id, false).unwrap();
        assert!(item.validate().is_err());
        set(&machine.directory, &rule.id, true).unwrap();
        store.settings.host = "edited.invalid".into();
        store.save(store.settings.forwards.clone()).unwrap();
        assert!(item.validate().is_err());
        store.settings.host = "fixture.invalid".into();
        let mut changed = rule.clone();
        changed.remote_port += 1;
        store.save(vec![changed]).unwrap();
        assert!(item.validate().is_err());
        store.save(Vec::new()).unwrap();
        assert!(item.validate().is_err());
        let (queue, _) = collect(&[machine]);
        assert!(queue.is_empty(), "Deleted IDs must be inert");
    }
}
