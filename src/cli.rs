use crate::{
    background,
    machines::{self, Catalog, Machine},
    store::{self, Forward, Store},
};
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Parser)]
#[command(
    name = "ports",
    version,
    about = "Saved SSH forwards, shared across terminal views."
)]
pub struct Options {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long,global=true,default_value_os_t=store::default_directory())]
    pub data_dir: PathBuf,
    #[arg(long, global = true)]
    pub machine: Option<String>,
    #[arg(long, conflicts_with = "machine")]
    pub host: Option<String>,
    #[arg(long)]
    pub machines: bool,
    #[arg(long)]
    pub focus_existing: bool,
    #[arg(long)]
    pub foreground: bool,
    #[arg(long, hide = true)]
    pub serve: bool,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub stop_all: bool,
    #[arg(long)]
    pub stop_daemon: bool,
    #[command(subcommand)]
    pub command: Option<Action>,
}
#[derive(Debug, Subcommand)]
pub enum Action {
    Machines {
        #[command(subcommand)]
        action: MachineAction,
    },
    Doctor,
    List,
    RestartManager,
    StopAll,
    Save {
        #[arg(long,value_parser=store::port)]
        remote: u16,
        #[arg(long,value_parser=store::port)]
        local: Option<u16>,
        #[arg(long, default_value = "")]
        name: String,
    },
    Start {
        id: String,
        #[arg(long,default_value_t=5.0,value_parser=wait_seconds)]
        wait: f64,
    },
    Stop {
        id: String,
    },
    Delete {
        id: String,
        #[arg(long, required = true)]
        yes: bool,
    },
}
#[derive(Debug, Subcommand)]
pub enum MachineAction {
    List,
    Add {
        target: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long,value_parser=store::port)]
        ssh_port: Option<u16>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Discover {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Import {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        select: Vec<String>,
    },
    Pick {
        #[arg(long)]
        force_picker: bool,
        #[arg(long)]
        no_window_context: bool,
    },
}
fn wait_seconds(text: &str) -> std::result::Result<f64, String> {
    let value = text
        .parse::<f64>()
        .map_err(|_| "--wait must be between 0 and 30 seconds.")?;
    if value.is_finite() && (0.0..=30.0).contains(&value) {
        Ok(value)
    } else {
        Err("--wait must be between 0 and 30 seconds.".into())
    }
}
pub fn selected_machine(catalog: &Catalog, selector: Option<&str>) -> Result<Option<Machine>> {
    if let Some(selector) = selector {
        return catalog.get(selector).map(Some);
    }
    let machines = catalog.list()?;
    ensure!(
        machines.len() <= 1,
        "Several machines are saved. Add --machine ID; run machines list to choose one."
    );
    Ok(machines.into_iter().next())
}
pub fn listing(store: &Store) -> Result<Value> {
    let snapshot = if store.directory.join("endpoint.json").exists() {
        background::exchange(
            &store.directory,
            "status",
            json!({}),
            Duration::from_secs(2),
        )
        .ok()
    } else {
        None
    };
    let mut rules = snapshot
        .as_ref()
        .and_then(|s| s["forwards"].as_array())
        .cloned()
        .unwrap_or(
            serde_json::to_value(&store.settings.forwards)?
                .as_array()
                .unwrap()
                .clone(),
        );
    for rule in &mut rules {
        rule["state"] = json!(
            snapshot
                .as_ref()
                .map(|s| s["states"][rule["id"].as_str().unwrap_or("")]
                    .as_str()
                    .unwrap_or("OFF"))
                .unwrap_or("UNKNOWN")
        );
        rule["url"] = json!(format!("http://127.0.0.1:{}", rule["local_port"]));
    }
    Ok(
        json!({"host":store.settings.host,"background":if snapshot.is_some(){"connected"}else{"not_connected"},
        "warning":if snapshot.is_none()&&store.directory.join("endpoint.json").exists(){"Live status is unavailable. Existing connections might still be running."}else{""},"forwards":rules}),
    )
}
fn doctor(catalog: &Catalog) -> Value {
    let ssh = crate::process::ssh_executable();
    let saved = catalog.list();
    let ok = ssh.is_ok() && saved.is_ok();
    json!({"ok":ok,"checks":[
        {"name":"OpenSSH client","ok":ssh.is_ok(),"detail":ssh.map(|p|p.display().to_string()).unwrap_or_else(|e|e.to_string())},
        {"name":"Saved machines","ok":saved.is_ok(),"detail":saved.map(|m|format!("{} machines; no connection attempted",m.len())).unwrap_or_else(|e|e.to_string())},
        {"name":"Runtime","ok":true,"detail":format!("Ports {} native Rust / {} / {}",env!("CARGO_PKG_VERSION"),std::env::consts::OS,std::env::consts::ARCH)}]})
}
pub fn execute(options: &Options) -> Result<Value> {
    let catalog = Catalog::new(&options.data_dir)?;
    let action = options.command.as_ref().context("Missing command")?;
    let mut command_name = match action {
        Action::Machines { .. } => "machines",
        Action::Doctor => "doctor",
        Action::List => "list",
        Action::RestartManager => "restart-manager",
        Action::StopAll => "stop-all",
        Action::Save { .. } => "save",
        Action::Start { .. } => "start",
        Action::Stop { .. } => "stop",
        Action::Delete { .. } => "delete",
    };
    let mut result = if let Action::Machines { action } = action {
        match action {
            MachineAction::List => json!({"machines":catalog.list()?}),
            MachineAction::Add {
                target,
                name,
                ssh_port,
                config,
            } => {
                let machine = catalog.add(target, name, *ssh_port, config.as_deref())?;
                json!({"machine":machine,"machines":catalog.list()?})
            }
            MachineAction::Discover { config } => {
                json!({"aliases":machines::ssh_aliases(config.as_deref())?})
            }
            MachineAction::Import { config, select } => {
                let imported = machines::import(
                    &catalog,
                    config.as_deref(),
                    if select.is_empty() {
                        None
                    } else {
                        Some(select)
                    },
                )?;
                json!({"imported":imported.into_iter().map(|m|m.id).collect::<Vec<_>>(),"machines":catalog.list()?})
            }
            MachineAction::Pick {
                force_picker,
                no_window_context,
            } => {
                command_name = "machines pick";
                let selected = crate::ui::choose_machine(
                    &catalog,
                    options.machine.as_deref(),
                    *force_picker,
                    !no_window_context,
                )?;
                let machine = selected.map(|machine| {
                    let mut value = serde_json::to_value(&machine).expect("Machine serialization");
                    value["directory"] = json!(machine.directory);
                    value
                });
                json!({"machine":machine})
            }
        }
    } else if matches!(action, Action::Doctor) {
        doctor(&catalog)
    } else {
        let saved = catalog.list();
        if matches!(action, Action::List)
            && options.machine.is_none()
            && saved.as_ref().is_ok_and(|machines| machines.len() > 1)
        {
            let mut summaries = Vec::new();
            let mut all = Vec::new();
            for machine in saved? {
                let mut summary = listing(&Store::load(&machine.directory)?)?;
                for rule in summary["forwards"]
                    .as_array()
                    .context("Invalid favorite list")?
                {
                    let mut rule = rule.clone();
                    rule["machine_id"] = json!(machine.id);
                    rule["machine_name"] = json!(machine.name);
                    all.push(rule);
                }
                summary["machine"] = serde_json::to_value(machine)?;
                summaries.push(summary);
            }
            json!({"machines":summaries,"forwards":all})
        } else {
            let directory = if matches!(action, Action::StopAll)
                && options.machine.is_none()
                && saved.is_err()
                && catalog.directory.join("endpoint.json").exists()
            {
                catalog.directory.clone()
            } else {
                selected_machine(&catalog, options.machine.as_deref())?
                    .map(|m| m.directory)
                    .unwrap_or(catalog.directory.clone())
            };
            if matches!(action, Action::StopAll) {
                let snapshot = background::exchange(
                    &directory,
                    "stop_all",
                    json!({}),
                    Duration::from_secs(5),
                )?;
                json!({"host":snapshot["host"],"background":"connected","forwards":snapshot["forwards"]})
            } else {
                let store = Store::load(&directory)?;
                if matches!(action, Action::List) {
                    listing(&store)?
                } else {
                    ensure!(
                        !store.settings.host.is_empty(),
                        "Add a machine first: ports machines add YOUR_SSH_NAME."
                    );
                    ensure!(
                        store.settings.keep_alive,
                        "This data folder uses foreground mode. Use its TUI to manage connections."
                    );
                    match action {
                        Action::RestartManager => {
                            let ids = background::restart(&directory)?;
                            let mut result = listing(&store)?;
                            result["restored_ids"] = json!(ids);
                            result
                        }
                        Action::Save {
                            remote,
                            local,
                            name,
                        } => {
                            store::name(name, true)?;
                            let snapshot = background::ensure_daemon(&directory)?;
                            let rules: Vec<Forward> =
                                serde_json::from_value(snapshot["forwards"].clone())?;
                            let previous = rules.into_iter().find(|rule| {
                                rule.local_port == local.unwrap_or(*remote)
                                    && rule.remote_port == *remote
                            });
                            let rule = if let Some(previous) = &previous {
                                let mut rule = previous.clone();
                                if !name.trim().is_empty() {
                                    rule.name = name.trim().into();
                                }
                                rule
                            } else {
                                Forward::new(local.unwrap_or(*remote), *remote, name)?
                            };
                            let result = background::exchange(
                                &directory,
                                "upsert",
                                json!({"rule":rule,"expected":previous}),
                                Duration::from_secs(5),
                            )?;
                            let mut listing = listing(&Store::load(&directory)?)?;
                            listing["id"] = result["rule_id"].clone();
                            listing
                        }
                        Action::Start { id, .. }
                        | Action::Stop { id }
                        | Action::Delete { id, .. } => {
                            let rule = store
                                .settings
                                .forwards
                                .iter()
                                .find(|r| r.id == *id)
                                .context("Favorite ID not found. Run list and use its exact ID.")?;
                            let (verb, args) = match action {
                                Action::Start { .. } => ("start", json!({"rule_id":id})),
                                Action::Stop { .. } => ("stop", json!({"rule_id":id})),
                                _ => ("delete", json!({"rule_id":id,"expected":rule})),
                            };
                            let mut snapshot = background::call(&directory, verb, args)?;
                            if let Action::Start { wait, .. } = action {
                                let deadline = Instant::now() + Duration::from_secs_f64(*wait);
                                while snapshot["states"][id] == "CONNECTING"
                                    && Instant::now() < deadline
                                {
                                    thread::sleep(Duration::from_millis(100));
                                    snapshot = background::exchange(
                                        &directory,
                                        "status",
                                        json!({}),
                                        Duration::from_secs(2),
                                    )?;
                                }
                                ensure!(
                                    snapshot["states"][id] != "ERROR",
                                    "{}",
                                    snapshot["details"][id]
                                        .as_str()
                                        .unwrap_or("SSH could not open this connection.")
                                );
                            }
                            let mut result = listing(&Store::load(&directory)?)?;
                            result["id"] = json!(id);
                            result
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    };
    result["schema_version"] = json!(1);
    if result.get("ok").is_none() {
        result["ok"] = json!(true);
    }
    result["command"] = json!(command_name);
    Ok(result)
}
pub fn print(result: &Value, as_json: bool) {
    if as_json {
        println!("{result}");
        return;
    }
    if let Some(error) = result.get("error") {
        eprintln!("{}", error["message"].as_str().unwrap_or("Command failed"));
        return;
    }
    if let Some(checks) = result["checks"].as_array() {
        for check in checks {
            println!(
                "{}: {} — {}",
                if check["ok"] == true { "OK" } else { "FAIL" },
                check["name"].as_str().unwrap_or(""),
                check["detail"].as_str().unwrap_or("")
            );
        }
    }
    for alias in result["aliases"].as_array().into_iter().flatten() {
        println!("{}", alias.as_str().unwrap_or(""));
    }
    for machine in result["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m.get("target").is_some())
    {
        println!(
            "{} | {} | id {}",
            machine["name"].as_str().unwrap_or(""),
            machine["target"].as_str().unwrap_or(""),
            machine["id"].as_str().unwrap_or("")
        );
    }
    for rule in result["forwards"].as_array().into_iter().flatten() {
        println!(
            "{:10} {} | local {} → remote {} | id {}",
            rule["state"].as_str().unwrap_or("OFF"),
            rule["name"].as_str().unwrap_or(""),
            rule["local_port"],
            rule["remote_port"],
            rule["id"].as_str().unwrap_or("")
        );
    }
    if let Some(warning) = result["warning"].as_str().filter(|s| !s.is_empty()) {
        println!("{warning}");
    }
    if let Some(id) = result["id"].as_str() {
        println!("Favorite ID: {id}");
    }
}
