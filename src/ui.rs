use crate::{
    auto_open::{self, Pending, Preferences},
    background::{self, Supervisor},
    forward_form,
    machines::{Catalog, Machine},
    process::NativeBackend,
    screen::{self, ACCENT, Input, MUTED},
    store::{self, Forward, Lock, Store},
    views,
};
use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    widgets::{Clear, Paragraph, Wrap},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Entry {
    pub machine: Machine,
    pub rule: Option<Forward>,
    pub state: String,
    pub details: String,
    pub open_automatically: bool,
}
fn entries(machine: &Machine, snapshot: Option<&Value>, error: Option<&str>) -> Result<Vec<Entry>> {
    let store = Store::load(&machine.directory)?;
    let preferences = Preferences::load(&machine.directory);
    let rules: Vec<Forward> = if let Some(snapshot) = snapshot {
        serde_json::from_value(snapshot["forwards"].clone())?
    } else {
        store.settings.forwards
    };
    let mut entries = rules
        .into_iter()
        .map(|rule| {
            let state = snapshot
                .map(|s| s["states"][&rule.id].as_str().unwrap_or("OFF"))
                .unwrap_or(if error.is_some() { "UNKNOWN" } else { "OFF" })
                .to_owned();
            let mut details = snapshot
                .and_then(|s| s["details"][&rule.id].as_str())
                .unwrap_or(error.unwrap_or(""))
                .to_owned();
            if let Err(error) = &preferences {
                details.push_str(&format!("\nAutomatic opening disabled: {error:#}"));
            }
            let open_automatically = preferences.as_ref().is_ok_and(|p| p.enabled(&rule.id));
            Entry {
                machine: machine.clone(),
                rule: Some(rule),
                state,
                details,
                open_automatically,
            }
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        entries.push(Entry {
            machine: machine.clone(),
            rule: None,
            state: String::new(),
            details: error.unwrap_or("").into(),
            open_automatically: false,
        });
    }
    Ok(entries)
}
fn load_entries(catalog: &Catalog, live: bool) -> Result<Vec<Entry>> {
    let machines = catalog.list()?;
    let mut all = Vec::new();
    for chunk in machines.chunks(4) {
        let rows = thread::scope(|scope| {
            let handles = chunk
                .iter()
                .map(|machine| {
                    scope.spawn(move || {
                        let response = if live && machine.directory.join("endpoint.json").exists() {
                            Some(background::exchange(
                                &machine.directory,
                                "status",
                                json!({}),
                                Duration::from_millis(500),
                            ))
                        } else {
                            None
                        };
                        let snapshot = response.as_ref().and_then(|r| r.as_ref().ok());
                        let error = response
                            .as_ref()
                            .and_then(|r| r.as_ref().err())
                            .map(|error| format!("Live status unavailable: {error:#}"));
                        entries(machine, snapshot, error.as_deref())
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .map_err(|_| anyhow::anyhow!("Status worker stopped"))?
                })
                .collect::<Result<Vec<_>>>()
        })?;
        for rows in rows {
            all.extend(rows);
        }
    }
    Ok(all)
}
#[derive(Clone)]
struct Operation {
    directory: PathBuf,
    command: &'static str,
    args: Value,
    automatic: Option<Pending>,
}
enum Update {
    Rows(u64, Result<Vec<Entry>>),
    Changed(Result<Value>),
}
struct Worker {
    polls: SyncSender<u64>,
    changes: SyncSender<Operation>,
    receiver: Receiver<Update>,
}
impl Worker {
    fn new(catalog: Catalog) -> Self {
        let (polls, jobs) = mpsc::sync_channel(1);
        let (changes, operations) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::channel();
        let poll_results = results.clone();
        let poll_catalog = catalog.clone();
        thread::spawn(move || {
            while let Ok(generation) = jobs.recv() {
                if poll_results
                    .send(Update::Rows(generation, load_entries(&poll_catalog, true)))
                    .is_err()
                {
                    break;
                }
            }
        });
        thread::spawn(move || {
            while let Ok(operation) = operations.recv() {
                if results
                    .send(Update::Changed(perform(&catalog, &operation)))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            polls,
            changes,
            receiver,
        }
    }
}
fn perform(catalog: &Catalog, operation: &Operation) -> Result<Value> {
    if let Some(automatic) = &operation.automatic {
        automatic.validate()?;
        // Controller preparation may wait; frozen checks must run after it.
        background::ensure_daemon(&operation.directory)?;
    }
    let rows = if matches!(operation.command, "start" | "restart" | "upsert" | "quick") {
        Some(load_entries(catalog, true)?)
    } else {
        None
    };
    let resolved = resolve_operation(operation, rows.as_deref().unwrap_or_default())?;
    let operation = &resolved;
    if let Some(automatic) = &operation.automatic {
        // Status can take time. Revalidate the frozen rule/route immediately
        // before dispatch too, without teaching old controllers new fields.
        automatic.validate()?;
        if rows.as_ref().is_some_and(|rows| {
            rows.iter().any(|row| {
                row.machine.directory == operation.directory
                    && row
                        .rule
                        .as_ref()
                        .is_some_and(|rule| rule.id == automatic.rule.id)
                    && requested(&row.state)
            })
        }) {
            return Ok(
                json!({"notice":format!("{} is already requested; left running.", automatic.rule.name)}),
            );
        }
    }
    if let Some(rows) = &rows {
        check_port_conflict(operation, rows)?;
    }
    if operation.command == "stop_listed" {
        let mut errors = Vec::new();
        for row in operation.args["machines"]
            .as_array()
            .context("Missing listed machines")?
        {
            let directory = PathBuf::from(
                row["directory"]
                    .as_str()
                    .context("Missing machine directory")?,
            );
            if directory.join("endpoint.json").exists()
                && let Err(error) =
                    background::exchange(&directory, "stop_all", json!({}), Duration::from_secs(5))
            {
                errors.push(format!(
                    "{}: {error:#}",
                    row["name"].as_str().unwrap_or("Machine")
                ));
            }
        }
        anyhow::ensure!(
            errors.is_empty(),
            "Some servers could not be stopped: {}",
            errors.join("; ")
        );
        Ok(json!({"ok":true}))
    } else if operation.command == "restart" {
        background::call(&operation.directory, "stop", operation.args.clone())?;
        background::call(&operation.directory, "start", operation.args.clone())
    } else if operation.automatic.is_some() {
        background::exchange(
            &operation.directory,
            operation.command,
            operation.args.clone(),
            Duration::from_secs(5),
        )
    } else {
        background::call(
            &operation.directory,
            operation.command,
            operation.args.clone(),
        )
    }
}
fn resolve_operation(operation: &Operation, rows: &[Entry]) -> Result<Operation> {
    if operation.command != "quick" {
        return Ok(operation.clone());
    }
    let entry = rows
        .iter()
        .find(|row| row.machine.directory == operation.directory)
        .context("This machine is no longer available. Reopen the machine picker.")?;
    quick_operation(
        entry,
        rows,
        operation.args["text"]
            .as_str()
            .context("Missing quick entry")?,
    )
}
// An enabled connection reserves its local port in this manager even while its
// network is down. Re-read live status near dispatch, rather than trusting the
// last painted frame. The SSH process remains the final OS-level bind check.
fn check_port_conflict(operation: &Operation, rows: &[Entry]) -> Result<()> {
    let rule = if operation.command == "upsert" {
        let rule: Forward = serde_json::from_value(operation.args["rule"].clone())?;
        let active = rows.iter().any(|row| {
            row.machine.directory == operation.directory
                && row.rule.as_ref().is_some_and(|old| old.id == rule.id)
                && requested(&row.state)
        });
        if operation.args["start"] != true && !active {
            return Ok(());
        }
        Some(rule)
    } else if matches!(operation.command, "start" | "restart") {
        Store::load(&operation.directory)?
            .settings
            .forwards
            .into_iter()
            .find(|rule| operation.args["rule_id"] == rule.id)
    } else {
        None
    };
    if let Some(rule) = rule
        && let Some(other) = rows.iter().find(|row| {
            row.machine.directory != operation.directory
                && requested(&row.state)
                && row
                    .rule
                    .as_ref()
                    .is_some_and(|other| other.local_port == rule.local_port)
        })
    {
        anyhow::bail!(
            "Local port {} is already requested by {} ({}). Stop that connection or choose another local port.",
            rule.local_port,
            other.machine.name,
            other.state
        );
    }
    Ok(())
}
fn quick_operation(entry: &Entry, rows: &[Entry], text: &str) -> Result<Operation> {
    let text = text.trim();
    let split = text.find(char::is_whitespace).unwrap_or(text.len());
    let (local_port, remote_port) = store::quick_ports(&text[..split])?;
    let label = text[split..].trim();
    let previous = rows
        .iter()
        .filter(|row| row.machine.id == entry.machine.id)
        .filter_map(|row| row.rule.as_ref())
        .find(|rule| {
            !rule.is_socks()
                && rule.local_port == local_port
                && rule.remote_port == Some(remote_port)
        });
    let mut rule = if let Some(previous) = previous {
        previous.clone()
    } else {
        Forward::new(local_port, remote_port, label)?
    };
    if !label.is_empty() {
        rule.name = store::name(label, false)?;
    }
    let operation = Operation {
        directory: entry.machine.directory.clone(),
        command: "upsert",
        args: json!({"rule":rule,"expected":previous,"start":true}),
        automatic: None,
    };
    check_port_conflict(&operation, rows)?;
    Ok(operation)
}
pub fn choose_machine(
    catalog: &Catalog,
    selector: Option<&str>,
    force: bool,
    window_context: bool,
) -> Result<Option<Machine>> {
    if let Some(selector) = selector {
        return catalog.get(selector).map(Some);
    }
    if !force
        && window_context
        && cfg!(windows)
        && std::env::var_os("WT_SESSION").is_some()
        && let Some(id) = views::machine_for_invocation(&catalog.directory)?
        && let Ok(machine) = catalog.get(&id)
    {
        return Ok(Some(machine));
    }
    let machines = catalog.list()?;
    if !force && machines.len() == 1 {
        return Ok(machines.into_iter().next());
    }
    let mut session = screen::Session::enter()?;
    crate::picker::choose(&mut session.terminal, catalog)
}
fn requested(state: &str) -> bool {
    matches!(state, "ON" | "CONNECTING" | "RETRYING")
}
fn identity(entry: &Entry) -> (String, Option<String>) {
    (
        entry.machine.id.clone(),
        entry.rule.as_ref().map(|r| r.id.clone()),
    )
}
fn select_previous(
    rows: &[Entry],
    previous: Option<(String, Option<String>)>,
    fallback: usize,
) -> usize {
    previous
        .as_ref()
        .and_then(|wanted| rows.iter().position(|e| identity(e) == *wanted))
        .unwrap_or(fallback.min(rows.len().saturating_sub(1)))
}
pub struct Presentation<'a> {
    pub rows: &'a [Entry],
    pub selected: usize,
    pub quick: &'a Input,
    pub typing: bool,
    pub notice: &'a str,
    pub busy: bool,
    pub persistent: bool,
    pub automatic: &'a VecDeque<Pending>,
    pub automatic_errors: &'a BTreeMap<(PathBuf, String), String>,
}
pub fn render(frame: &mut ratatui::Frame, view: &Presentation<'_>) {
    render_named(frame, view, None);
}
fn render_named(
    frame: &mut ratatui::Frame,
    view: &Presentation<'_>,
    names: Option<&crate::service_name::Inspector>,
) {
    let Presentation {
        rows,
        selected,
        quick,
        typing,
        notice,
        busy,
        persistent,
        automatic,
        automatic_errors,
    } = *view;
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .margin(1)
    .split(frame.area());
    let active = rows
        .get(selected)
        .map(|r| r.machine.name.as_str())
        .unwrap_or("Choose a machine");
    let mode = if persistent {
        "Background on · closing this view keeps forwards running"
    } else {
        "Foreground · closing this view stops its forwards"
    };
    frame.render_widget(
        Paragraph::new(format!("{active}  ·  {mode}")).block(screen::panel(&format!(
            " PORTS {} · {} ",
            crate::channel::CHANNEL.to_ascii_uppercase(),
            env!("CARGO_PKG_VERSION")
        ))),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{}{}", if typing { "› " } else { "" }, quick.value)).block(
            screen::panel(if typing {
                " Enter opens · PORT or LOCAL:REMOTE [name] · Esc returns "
            } else {
                " Quick forward · N or type a port · example: 8000 API "
            }),
        ),
        areas[1],
    );
    crate::connection_list::render_named(frame, areas[2], rows, selected, automatic, names);
    let detail = rows.get(selected).map(|entry| {
        if let Some(rule) = &entry.rule {
            if rule.is_socks() {
                let status = if entry.details.is_empty() { "Configure a SOCKS5 client; B / T shows how. ON confirms only the local listener." } else { &entry.details };
                format!("{} · SOCKS5 {} via {}\n{status}", rule.name, rule.endpoint_url(), entry.machine.target)
            } else if entry.details.is_empty() {
                format!("{} · {} → {}:{}\nON confirms the local SSH listener. The app on the server must also be running.", rule.name, rule.endpoint_url(), entry.machine.target, rule.remote_port.unwrap_or_default())
            } else {
                entry.details.clone()
            }
        } else { "Press A for a fixed forward or P for a SOCKS5 proxy.".into() }
    }).unwrap_or_default();
    let detail = format!(
        "Open automatically: {} (F2 settings)\n{detail}",
        rows.get(selected)
            .map_or("off", |entry| if entry.open_automatically {
                "on"
            } else {
                "off"
            })
    );
    let detail = if let Some(error) = rows.get(selected).and_then(|entry| {
        entry.rule.as_ref().and_then(|rule| {
            automatic_errors.get(&(entry.machine.directory.clone(), rule.id.clone()))
        })
    }) {
        format!("Automatic opening: {error}\n{detail}")
    } else {
        detail
    };
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED)),
        areas[3],
    );
    let automatic_summary = if automatic_errors.is_empty() {
        String::new()
    } else {
        format!(
            "{} automatic openings need attention; select a row for details. ",
            automatic_errors.len()
        )
    };
    frame.render_widget(Paragraph::new(format!("Enter on/off · N quick · A add · P proxy · E edit · D delete · B/T use · H hosts · F2 settings · ? help · Q close\n{automatic_summary}{}{}",if busy{"Working… "}else{""},notice)).wrap(Wrap{trim:false}).style(Style::default().fg(ACCENT)),areas[4]);
}
fn browser_url(rule: &Forward) -> Result<String> {
    anyhow::ensure!(
        !rule.is_socks(),
        "SOCKS5 is a proxy endpoint. Configure a proxy-aware client; it is not an HTTP page."
    );
    Ok(rule.endpoint_url())
}
fn open_browser(rule: &Forward) -> Result<()> {
    let url = browser_url(rule)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(0x08000000)
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(&url)
            .spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
    }
    Ok(())
}
fn socks_guidance(rule: &Forward) -> String {
    format!(
        "SOCKS5: 127.0.0.1:{} · Enter / Esc returns\nConfigure a SOCKS5 client to use this local port.\nEach request chooses a destination through the SSH server.\nExample curl (replace DESTINATION:PORT):\ncurl --socks5-hostname 127.0.0.1:{} http://DESTINATION:PORT/\nThat flag sends the hostname to the server for resolution.\nBrowser localhost bypass may need separate configuration.\nPorts changes no OS/browser proxy settings or remote app.\nTCP only; no UDP or whole-device VPN.\nON confirms the local listener, not a healthy destination.\nB / T send no HTTP request and launch no browser.",
        rule.local_port, rule.local_port
    )
}

fn settings(terminal: &mut screen::Screen, entry: &Entry) -> Result<bool> {
    let old_scope = views::read_scope(&entry.machine.directory)?;
    let old_auto = Preferences::load(&entry.machine.directory);
    let editable = entry.rule.is_some() && old_auto.is_ok();
    let mut scope = old_scope.clone();
    let original = entry
        .rule
        .as_ref()
        .is_some_and(|rule| old_auto.as_ref().is_ok_and(|p| p.enabled(&rule.id)));
    let mut enabled = original;
    let mut selected = usize::from(!editable);
    loop {
        terminal.draw(|frame| {
            let area = screen::centered(frame.area(), 76, 14);
            frame.render_widget(Clear, area);
            frame.render_widget(screen::panel(" Settings "), area);
            let body = area.inner(ratatui::layout::Margin::new(2, 1));
            let favorite = entry.rule.as_ref().map(|rule| rule.name.as_str()).unwrap_or("Choose a saved favorite first");
            let text = format!("{} / {favorite}\n\n{} [{}] Open automatically\n    Connect when a new Ports view opens.\n{} Return shortcut: {}\n\nStop keeps this preference for the next new view.\nSpace changes selection; arrows choose a setting.\nEnter saves; Esc cancels.",
                entry.machine.name, if selected == 0 { ">" } else { " " },
                if !editable { "unavailable" } else if enabled { "x" } else { " " },
                if selected == 1 { ">" } else { " " },
                if scope == "window" { "this window" } else { "across windows" });
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), body);
        })?;
        let Some(Event::Key(key)) = screen::key()? else {
            continue;
        };
        if screen::quit(key) || key.code == KeyCode::Esc {
            return Ok(false);
        }
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                selected = if selected == 0 || !editable { 1 } else { 0 };
            }
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                if selected == 0 {
                    enabled = !enabled;
                } else {
                    scope = if scope == "all" { "window" } else { "all" }.into();
                }
            }
            KeyCode::Enter => {
                if let Some(rule) = &entry.rule
                    && editable
                    && enabled != original
                {
                    auto_open::set(&entry.machine.directory, &rule.id, enabled)?;
                }
                if scope != old_scope {
                    views::save_scope(&entry.machine.directory, &scope)?;
                }
                return Ok(true);
            }
            _ => {}
        }
    }
}

fn automatic_operation(pending: Pending) -> Operation {
    Operation {
        directory: pending.machine.directory.clone(),
        command: "start",
        args: json!({"rule_id":pending.rule.id}),
        automatic: Some(pending),
    }
}

pub fn run(catalog: Catalog, machine: Machine, foreground: bool) -> Result<()> {
    crate::channel::validate_directory(&machine.directory)?;
    let store = Store::load(&machine.directory)?;
    let persistent = store.settings.keep_alive && !foreground;
    let _locks = if persistent {
        None
    } else {
        Some((
            Lock::acquire(&machine.directory, "manager.lock", Duration::ZERO)?,
            Lock::acquire(&machine.directory, "daemon.lock", Duration::ZERO)?,
        ))
    };
    let mut local = if persistent {
        None
    } else {
        Some(Supervisor::new(store, NativeBackend::new()?))
    };
    let worker = if persistent {
        Some(Worker::new(catalog.clone()))
    } else {
        None
    };
    let mut rows = if persistent {
        load_entries(&catalog, false)?
    } else {
        entries(&machine, None, None)?
    };
    let mut selected = rows
        .iter()
        .position(|entry| entry.machine.id == machine.id)
        .unwrap_or(0);
    let mut registration =
        if persistent && cfg!(windows) && std::env::var_os("WT_SESSION").is_some() {
            views::register_ports(
                &machine.directory,
                &catalog.directory,
                &machine.id,
                &machine.target,
            )
            .ok()
        } else {
            None
        };
    let mut registered_machine = machine.id.clone();
    let mut session = screen::Session::enter()?;
    // Focus-existing returned before this function. This queue is never rebuilt
    // by a poll, machine picker, settings save or controller reconnection.
    let displayed = if persistent {
        catalog.list()?
    } else {
        vec![machine.clone()]
    };
    let (mut automatic, warnings) = auto_open::collect(&displayed);
    let mut quick = Input::default();
    let mut typing = false;
    let mut notice = warnings.join("; ");
    let mut pending: VecDeque<Operation> = VecDeque::new();
    let mut polling = false;
    let mut changing = false;
    let mut active_automatic: Option<Pending> = None;
    let mut active_stop: Option<(PathBuf, String)> = None;
    let mut automatic_errors = BTreeMap::new();
    let mut closing = false;
    let mut next_poll = Instant::now();
    let mut generation = 0_u64;
    let mut names = crate::service_name::Inspector::default();
    loop {
        if !closing
            && !changing
            && pending.is_empty()
            && let Some(item) = automatic.pop_front()
        {
            pending.push_back(automatic_operation(item));
        }
        if let Some(worker) = &worker {
            if let Ok(update) = worker.receiver.try_recv() {
                match &update {
                    Update::Rows(..) => polling = false,
                    Update::Changed(..) => changing = false,
                }
                match update {
                    Update::Rows(epoch, _) if epoch != generation => {}
                    Update::Rows(_, Ok(updated)) => {
                        automatic_errors.retain(|(directory, id), _| {
                            updated.iter().any(|entry| {
                                entry.machine.directory == *directory
                                    && entry.rule.as_ref().is_some_and(|rule| rule.id == *id)
                                    && entry.state != "ON"
                            })
                        });
                        let before = rows.get(selected).map(identity);
                        selected = select_previous(&updated, before, selected);
                        rows = updated;
                    }
                    Update::Changed(Ok(snapshot)) => {
                        if let Some(item) = &active_automatic
                            && snapshot["states"][&item.rule.id] == "ERROR"
                        {
                            automatic_errors.insert(
                                (item.machine.directory.clone(), item.rule.id.clone()),
                                snapshot["details"][&item.rule.id]
                                    .as_str()
                                    .unwrap_or("Could not start the forward.")
                                    .into(),
                            );
                        }
                        generation = generation.wrapping_add(1);
                        notice = if let Some(message) = snapshot["notice"].as_str() {
                            message.into()
                        } else if let Some(item) = &active_automatic {
                            format!("Opening {} automatically.", item.rule.name)
                        } else if snapshot.get("rule_id").is_some() {
                            "Favorite saved".into()
                        } else {
                            "Updated".into()
                        };
                        next_poll = Instant::now();
                    }
                    Update::Changed(Err(error)) => {
                        if let Some(item) = &active_automatic {
                            automatic_errors.insert(
                                (item.machine.directory.clone(), item.rule.id.clone()),
                                format!("{error:#}"),
                            );
                        }
                        notice = format!("{error:#}");
                    }
                    Update::Rows(_, Err(error)) => notice = format!("{error:#}"),
                }
                if !changing {
                    active_automatic = None;
                    active_stop = None;
                }
            }
            if !changing && let Some(operation) = pending.pop_front() {
                generation = generation.wrapping_add(1);
                active_automatic = operation.automatic.clone();
                active_stop = (operation.command == "stop").then(|| {
                    (
                        operation.directory.clone(),
                        operation.args["rule_id"].as_str().unwrap_or("").to_owned(),
                    )
                });
                worker.changes.send(operation)?;
                changing = true;
            }
            if !polling && !changing && !closing && Instant::now() >= next_poll {
                worker.polls.send(generation)?;
                polling = true;
                next_poll = Instant::now() + Duration::from_millis(750);
            }
        } else if let Some(local) = &mut local {
            if let Some(operation) = pending.pop_front() {
                let result = (|| -> Result<Value> {
                    if let Some(automatic) = &operation.automatic {
                        automatic.validate()?;
                    }
                    let current = entries(&machine, Some(&local.snapshot()), None)?;
                    let operation = resolve_operation(&operation, &current)?;
                    if operation.command == "restart" {
                        local
                            .local("stop", operation.args.clone())
                            .and_then(|_| local.local("start", operation.args))
                    } else {
                        local.local(operation.command, operation.args)
                    }
                })();
                match result {
                    Ok(snapshot) => {
                        if let Some(item) = &operation.automatic
                            && snapshot["states"][&item.rule.id] == "ERROR"
                        {
                            automatic_errors.insert(
                                (item.machine.directory.clone(), item.rule.id.clone()),
                                snapshot["details"][&item.rule.id]
                                    .as_str()
                                    .unwrap_or("Could not start the forward.")
                                    .into(),
                            );
                        }
                        notice = "Updated".into();
                    }
                    Err(error) => {
                        if let Some(item) = &operation.automatic {
                            automatic_errors.insert(
                                (item.machine.directory.clone(), item.rule.id.clone()),
                                format!("{error:#}"),
                            );
                        }
                        notice = format!("{error:#}");
                    }
                }
            }
            if Instant::now() >= next_poll {
                local.manager.poll(Instant::now());
                let snapshot = local.snapshot();
                rows = entries(&machine, Some(&snapshot), None)?;
                automatic_errors.retain(|(directory, id), _| {
                    rows.iter().any(|entry| {
                        entry.machine.directory == *directory
                            && entry.rule.as_ref().is_some_and(|rule| rule.id == *id)
                            && entry.state != "ON"
                    })
                });
                selected = selected.min(rows.len().saturating_sub(1));
                next_poll = Instant::now() + Duration::from_millis(250);
            }
        }
        names.poll(&rows);
        if closing && !changing && pending.is_empty() {
            break;
        }
        if let Some(entry) = rows.get(selected)
            && registered_machine != entry.machine.id
        {
            if let Some(registration) = &mut registration {
                let _ = registration.select_machine(
                    &entry.machine.directory,
                    &catalog.directory,
                    &entry.machine.id,
                    &entry.machine.target,
                );
            }
            registered_machine = entry.machine.id.clone();
        }
        session.terminal.draw(|frame| {
            render_named(
                frame,
                &Presentation {
                    rows: &rows,
                    selected,
                    quick: &quick,
                    typing,
                    notice: &notice,
                    busy: changing || !pending.is_empty(),
                    persistent,
                    automatic: &automatic,
                    automatic_errors: &automatic_errors,
                },
                Some(&names),
            );
            names.draw(frame);
        })?;
        let event = screen::key()?;
        if matches!(event, Some(Event::Key(_) | Event::FocusGained))
            && let Some(registration) = &mut registration
        {
            let _ = registration.focused();
        }
        let Some(event) = event else {
            continue;
        };
        if let Event::Paste(text) = event {
            if typing {
                quick.insert(&text);
            }
            continue;
        }
        let Event::Key(key) = event else {
            continue;
        };
        if screen::quit(key) {
            names.close();
            automatic.clear();
            closing = true;
            continue;
        }
        if closing {
            continue;
        }
        if names.key(key) {
            continue;
        }
        let entry = rows.get(selected).cloned();
        if typing {
            match key.code {
                KeyCode::Esc => typing = false,
                KeyCode::Enter if pending.is_empty() && !changing => {
                    let result = (|| -> Result<Operation> {
                        let entry = entry.as_ref().context("Choose a machine first.")?;
                        // Validate immediately, but resolve an existing mapping
                        // again on the worker. A fast next entry can precede the
                        // screen's refresh after the last saved favorite.
                        quick_operation(entry, &rows, &quick.value)?;
                        Ok(Operation {
                            directory: entry.machine.directory.clone(),
                            command: "quick",
                            args: json!({"text":quick.value}),
                            automatic: None,
                        })
                    })();
                    match result {
                        Ok(operation) => {
                            pending.push_back(operation);
                            quick.clear();
                            typing = false;
                        }
                        Err(error) => notice = format!("{error:#}"),
                    }
                }
                _ => quick.key(key),
            }
            continue;
        }
        if let Some(entry) = &entry
            && let Some(rule) = &entry.rule
        {
            let queued = automatic
                .iter()
                .any(|item| item.matches(&entry.machine, rule));
            let starting = active_automatic
                .as_ref()
                .is_some_and(|item| item.matches(&entry.machine, rule));
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
                && (queued || starting || (changing && requested(&entry.state)))
            {
                automatic.retain(|item| !item.matches(&entry.machine, rule));
                if starting || requested(&entry.state) {
                    let key = (entry.machine.directory.clone(), rule.id.clone());
                    if active_stop.as_ref() != Some(&key)
                        && !pending.iter().any(|operation| {
                            operation.command == "stop"
                                && operation.directory == key.0
                                && operation.args["rule_id"] == key.1
                        })
                    {
                        pending.push_back(Operation {
                            directory: entry.machine.directory.clone(),
                            command: "stop",
                            args: json!({"rule_id":rule.id}),
                            automatic: None,
                        });
                    }
                    notice = "Stopping after the current request finishes.".into();
                } else {
                    notice = "Automatic opening cancelled for this view; preference kept.".into();
                }
                continue;
            }
            if matches!(key.code, KeyCode::Char('r' | 'd')) {
                automatic.retain(|item| !item.matches(&entry.machine, rule));
            }
        }
        match key.code {
            KeyCode::Char('q') => {
                automatic.clear();
                closing = true;
            }
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(rows.len().saturating_sub(1))
            }
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::PageDown => selected = (selected + 10).min(rows.len().saturating_sub(1)),
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = rows.len().saturating_sub(1),
            KeyCode::Char('n') => typing = true,
            KeyCode::Char(c) if c.is_ascii_digit() => {
                typing = true;
                quick.insert(&c.to_string());
            }
            KeyCode::Char('?') => {
                screen::message(&mut session.terminal, " Ports keyboard ", HELP, false)?;
            }
            KeyCode::Char('h') if persistent => {
                if let Some(machine) = crate::picker::choose(&mut session.terminal, &catalog)? {
                    generation = generation.wrapping_add(1);
                    rows = load_entries(&catalog, false)?;
                    selected = rows
                        .iter()
                        .position(|entry| entry.machine.id == machine.id)
                        .unwrap_or(0);
                    next_poll = Instant::now();
                }
            }
            KeyCode::F(2) => {
                if let Some(entry) = &entry {
                    match settings(&mut session.terminal, entry) {
                        Ok(true) => {
                            notice =
                                "Settings saved. Automatic opening applies to the next new view."
                                    .into();
                            next_poll = Instant::now();
                        }
                        Ok(false) => {}
                        Err(error) => notice = format!("{error:#}"),
                    }
                }
            }
            KeyCode::Char('b') => {
                if let Some(rule) = entry.as_ref().and_then(|e| e.rule.as_ref()) {
                    if rule.is_socks() {
                        screen::message(
                            &mut session.terminal,
                            " SOCKS5 client setup ",
                            &socks_guidance(rule),
                            false,
                        )?;
                    } else if let Err(error) = open_browser(rule) {
                        notice = error.to_string();
                    }
                }
            }
            _ if (!pending.is_empty() || changing) && key.code != KeyCode::Char('s') => {
                notice = "Wait for the current change to finish.".into()
            }
            KeyCode::Char('t') => {
                if let Some(entry) = &entry {
                    if let Some(rule) = entry.rule.as_ref().filter(|rule| rule.is_socks()) {
                        screen::message(
                            &mut session.terminal,
                            " SOCKS5 client setup ",
                            &socks_guidance(rule),
                            false,
                        )?;
                    } else if let Err(error) = names.open(entry, persistent) {
                        notice = format!("{error:#}");
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('r') => {
                if let Some(entry) = &entry
                    && let Some(rule) = &entry.rule
                {
                    pending.push_back(Operation {
                        directory: entry.machine.directory.clone(),
                        command: if key.code == KeyCode::Char('r') {
                            "restart"
                        } else if requested(&entry.state) {
                            "stop"
                        } else {
                            "start"
                        },
                        args: json!({"rule_id":rule.id}),
                        automatic: None,
                    });
                }
            }
            KeyCode::Char('e') => {
                if let Some(entry) = &entry
                    && let Some(previous) = &entry.rule
                {
                    names.reset(entry);
                    let directory = entry.machine.directory.clone();
                    let model = forward_form::Model::new(&directory, previous.clone());
                    let result = forward_form::edit(
                        &mut session.terminal,
                        &entry.machine.name,
                        model,
                        |request| {
                            forward_form::save(&directory, request, |rule, expected| {
                                let args = json!({"rule":rule,"expected":expected,"start":false});
                                if let Some(local) = &mut local {
                                    local.local("upsert", args)
                                } else {
                                    perform(
                                        &catalog,
                                        &Operation {
                                            directory: directory.clone(),
                                            command: "upsert",
                                            args,
                                            automatic: None,
                                        },
                                    )
                                }
                            })
                        },
                    )?;
                    if let Some(message) = result {
                        notice = message;
                        generation = generation.wrapping_add(1);
                        next_poll = Instant::now();
                    }
                }
            }
            KeyCode::Char('a' | 'p') => {
                if let Some(entry) = &entry {
                    let socks = key.code == KeyCode::Char('p');
                    let fields = if socks {
                        vec![
                            ("Proxy port on this computer", "1080".into()),
                            ("Name (optional)", String::new()),
                        ]
                    } else {
                        vec![
                            ("App port on server", String::new()),
                            ("Port on this computer (blank uses same)", String::new()),
                            ("Name (optional)", String::new()),
                        ]
                    };
                    if let Some(values) = screen::form(
                        &mut session.terminal,
                        if socks {
                            " Add SOCKS5 proxy · BETA "
                        } else {
                            " Add favorite "
                        },
                        &fields,
                        0,
                        &format!(
                            "Server: {} · Save and connect{}",
                            entry.machine.name,
                            if socks {
                                "\nDestinations are chosen by your proxy client."
                            } else {
                                ""
                            }
                        ),
                    )? {
                        let result = (|| -> Result<Forward> {
                            if socks {
                                return Forward::socks(store::port(values[0].trim())?, &values[1]);
                            }
                            let remote = store::port(values[0].trim())?;
                            let local = if values[1].trim().is_empty() {
                                remote
                            } else {
                                store::port(values[1].trim())?
                            };
                            Forward::new(local, remote, &values[2])
                        })();
                        match result {
                            Ok(rule) => pending.push_back(Operation {
                                directory: entry.machine.directory.clone(),
                                command: "upsert",
                                args: json!({"rule":rule,"expected":null,"start":true}),
                                automatic: None,
                            }),
                            Err(error) => notice = error.to_string(),
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(entry) = &entry
                    && let Some(rule) = &entry.rule
                    && screen::message(
                        &mut session.terminal,
                        " Delete favorite ",
                        &format!(
                            "Delete {} from {} and stop its connection?",
                            rule.name, entry.machine.name
                        ),
                        true,
                    )?
                {
                    pending.push_back(Operation {
                        directory: entry.machine.directory.clone(),
                        command: "delete",
                        args: json!({"rule_id":rule.id,"expected":rule}),
                        automatic: None,
                    });
                }
            }
            KeyCode::Char('s') => {
                if let Some(entry) = &entry
                    && screen::message(
                        &mut session.terminal,
                        " Stop connections ",
                        &if persistent {
                            "Stop all forwards and pending retries on every listed server?".into()
                        } else {
                            format!("Stop all forwards for {}?", entry.machine.name)
                        },
                        true,
                    )?
                {
                    automatic.clear();
                    pending.push_back(Operation {
                        directory: entry.machine.directory.clone(),
                        command: if persistent {
                            "stop_listed"
                        } else {
                            "stop_all"
                        },
                        args: if persistent {
                            let machines=rows.iter().map(|row|(row.machine.id.clone(),json!({"name":row.machine.name,"directory":row.machine.directory}))).collect::<std::collections::BTreeMap<_,_>>();
                            json!({"machines":machines.into_values().collect::<Vec<_>>()})
                        } else {
                            json!({})
                        },
                        automatic: None,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}
const HELP: &str = "Ports BETA: isolated saved SSH connections.\nUp/Down select · Enter/Space start/stop · R reconnect\nN / digits quick fixed forward: 8000 or 18000:8000 API\nA add fixed forward · P add SOCKS5 proxy (default 1080)\nE edits the selected kind's ports/name + Open automatically\nD delete · S stop every listed server · H machines\nF2 settings · Q / Ctrl+Q close · ? help\n\nFixed forward: B opens HTTP; T previews title; U labels this view.\nSOCKS5: B / T shows client setup; neither sends HTTP nor opens a browser.\nSOCKS needs proxy-aware clients; OS/browser settings are unchanged.\n\nAuto opening defaults off; only a new view applies opted-in favorites.\nEnter cancels QUEUED; Q/S cancel unsent work. Stop cancels retries.\nRetries reach 30s; auth, host-key and occupied-port errors need attention.\nBackground survives terminal close; sign-out/reboot ends it.\n--foreground stops owned connections when closed.\nON confirms the local SSH listener, not every destination.\nBeta protocol 2 does not share stable controllers.";

#[cfg(test)]
mod socks_tests {
    use super::*;
    #[test]
    fn quick_entry_cannot_retarget_a_saved_proxy_with_the_same_local_port() {
        let temp = tempfile::tempdir().unwrap();
        let proxy = Forward::socks(1080, "Proxy").unwrap();
        let mut entry = Entry {
            machine: Machine {
                id: "fixture".into(),
                name: "Fixture".into(),
                target: "fixture.invalid".into(),
                directory: temp.path().to_path_buf(),
                ssh_port: None,
                ssh_config: None,
            },
            rule: Some(proxy.clone()),
            state: "OFF".into(),
            details: String::new(),
            open_automatically: false,
        };
        let operation = quick_operation(&entry, &[entry.clone()], "1080 API").unwrap();
        let fixed: Forward = serde_json::from_value(operation.args["rule"].clone()).unwrap();
        assert!(!fixed.is_socks());
        assert_eq!(fixed.remote_port, Some(1080));
        assert_ne!(fixed.id, proxy.id);
        assert!(operation.args["expected"].is_null());
        entry.rule = Some(fixed.clone());
        let operation = quick_operation(&entry, &[entry.clone()], "1080 renamed").unwrap();
        assert_eq!(operation.args["rule"]["id"], fixed.id);
        assert!(browser_url(&proxy).is_err());
        assert_eq!(browser_url(&fixed).unwrap(), "http://127.0.0.1:1080");
    }
}
