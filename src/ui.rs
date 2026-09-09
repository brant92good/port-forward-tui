use crate::{
    background::{self, Supervisor},
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
    style::{Color, Style},
    widgets::{Cell, Paragraph, Row, Table, TableState, Wrap},
};
use serde_json::{Value, json};
use std::{
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
}
fn entries(machine: &Machine, snapshot: Option<&Value>, error: Option<&str>) -> Result<Vec<Entry>> {
    let store = Store::load(&machine.directory)?;
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
            let details = snapshot
                .and_then(|s| s["details"][&rule.id].as_str())
                .unwrap_or(error.unwrap_or(""))
                .to_owned();
            Entry {
                machine: machine.clone(),
                rule: Some(rule),
                state,
                details,
            }
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        entries.push(Entry {
            machine: machine.clone(),
            rule: None,
            state: String::new(),
            details: error.unwrap_or("").into(),
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
    let rows = if matches!(operation.command, "start" | "restart" | "upsert" | "quick") {
        Some(load_entries(catalog, true)?)
    } else {
        None
    };
    let resolved = resolve_operation(operation, rows.as_deref().unwrap_or_default())?;
    let operation = &resolved;
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
        .find(|rule| rule.local_port == local_port && rule.remote_port == remote_port);
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
}
pub fn render(frame: &mut ratatui::Frame, view: &Presentation<'_>) {
    let Presentation {
        rows,
        selected,
        quick,
        typing,
        notice,
        busy,
        persistent,
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
        Paragraph::new(format!("{active}  ·  {mode}")).block(screen::panel(" PORTS ")),
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
    let table = rows.iter().map(|entry| {
        let color = match entry.state.as_str() {
            "ON" => Color::Green,
            "CONNECTING" | "RETRYING" => Color::Yellow,
            "ERROR" => Color::Red,
            _ => MUTED,
        };
        let (name, local, remote) = entry
            .rule
            .as_ref()
            .map(|r| {
                (
                    r.name.clone(),
                    r.local_port.to_string(),
                    r.remote_port.to_string(),
                )
            })
            .unwrap_or((
                "No favorites — press A".into(),
                String::new(),
                String::new(),
            ));
        Row::new([
            Cell::from(entry.machine.name.clone()).style(Style::default().fg(ACCENT)),
            Cell::from(entry.state.clone()).style(Style::default().fg(color)),
            Cell::from(name),
            Cell::from(local),
            Cell::from("→"),
            Cell::from(remote),
        ])
    });
    frame.render_stateful_widget(
        Table::new(
            table,
            [
                Constraint::Percentage(25),
                Constraint::Length(11),
                Constraint::Min(12),
                Constraint::Length(7),
                Constraint::Length(2),
                Constraint::Length(7),
            ],
        )
        .header(
            Row::new(["SERVER", "STATE", "FAVORITE", "LOCAL", "", "REMOTE"])
                .style(Style::default().fg(MUTED)),
        )
        .block(screen::panel(" Saved connections "))
        .row_highlight_style(Style::default().bg(Color::Rgb(37, 49, 67)))
        .highlight_symbol("› "),
        areas[2],
        &mut TableState::default().with_selected(Some(selected)),
    );
    let detail=rows.get(selected).map(|entry|if entry.details.is_empty(){entry.rule.as_ref().map(|r|format!("{} · http://127.0.0.1:{} → {}:{}\nON confirms the local SSH listener. The app on the server must also be running.",r.name,r.local_port,entry.machine.target,r.remote_port)).unwrap_or_else(||"Press A to add a favorite for this machine.".into())}else{entry.details.clone()}).unwrap_or_default();
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED)),
        areas[3],
    );
    frame.render_widget(Paragraph::new(format!("Enter on/off · N quick · A add · E edit · D delete · B browser · H machines · ? help · Q close\n{}{}",if busy{"Working… "}else{""},notice)).wrap(Wrap{trim:false}).style(Style::default().fg(ACCENT)),areas[4]);
}
fn open_browser(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}");
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
pub fn run(catalog: Catalog, machine: Machine, foreground: bool) -> Result<()> {
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
    let mut quick = Input::default();
    let mut typing = false;
    let mut notice = String::new();
    let mut pending: Option<Operation> = None;
    let mut polling = false;
    let mut changing = false;
    let mut closing = false;
    let mut next_poll = Instant::now();
    let mut generation = 0_u64;
    loop {
        if let Some(worker) = &worker {
            if let Ok(update) = worker.receiver.try_recv() {
                match &update {
                    Update::Rows(..) => polling = false,
                    Update::Changed(..) => changing = false,
                }
                match update {
                    Update::Rows(epoch, _) if epoch != generation => {}
                    Update::Rows(_, Ok(updated)) => {
                        let before = rows.get(selected).map(identity);
                        selected = select_previous(&updated, before, selected);
                        rows = updated;
                    }
                    Update::Changed(Ok(snapshot)) => {
                        generation = generation.wrapping_add(1);
                        notice = if snapshot.get("rule_id").is_some() {
                            "Favorite saved".into()
                        } else {
                            "Updated".into()
                        };
                        next_poll = Instant::now();
                    }
                    Update::Rows(_, Err(error)) | Update::Changed(Err(error)) => {
                        notice = format!("{error:#}")
                    }
                }
            }
            if !changing && let Some(operation) = pending.take() {
                generation = generation.wrapping_add(1);
                worker.changes.send(operation)?;
                changing = true;
            }
            if !polling && !changing && !closing && Instant::now() >= next_poll {
                worker.polls.send(generation)?;
                polling = true;
                next_poll = Instant::now() + Duration::from_millis(750);
            }
        } else if let Some(local) = &mut local {
            if let Some(operation) = pending.take() {
                let result = (|| -> Result<Value> {
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
                    Ok(_) => notice = "Updated".into(),
                    Err(error) => notice = format!("{error:#}"),
                }
            }
            if Instant::now() >= next_poll {
                local.manager.poll(Instant::now());
                let snapshot = local.snapshot();
                rows = entries(&machine, Some(&snapshot), None)?;
                selected = selected.min(rows.len().saturating_sub(1));
                next_poll = Instant::now() + Duration::from_millis(250);
            }
        }
        if closing && !changing && pending.is_none() {
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
            render(
                frame,
                &Presentation {
                    rows: &rows,
                    selected,
                    quick: &quick,
                    typing,
                    notice: &notice,
                    busy: changing || pending.is_some(),
                    persistent,
                },
            )
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
            closing = true;
            continue;
        }
        if closing {
            continue;
        }
        let entry = rows.get(selected).cloned();
        if typing {
            match key.code {
                KeyCode::Esc => typing = false,
                KeyCode::Enter if pending.is_none() && !changing => {
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
                        })
                    })();
                    match result {
                        Ok(operation) => {
                            pending = Some(operation);
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
        match key.code {
            KeyCode::Char('q') => closing = true,
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
                    let scope = views::read_scope(&entry.machine.directory)
                        .unwrap_or_else(|_| "all".into());
                    if let Some(values) = screen::form(
                        &mut session.terminal,
                        " Return shortcut scope ",
                        &[("all = across windows; window = this window", scope)],
                        0,
                        "Windows Terminal: return to the last-used matching view.",
                    )? {
                        match views::save_scope(&entry.machine.directory, values[0].trim()) {
                            Ok(()) => notice = "Return shortcut preference saved.".into(),
                            Err(error) => notice = error.to_string(),
                        }
                    }
                }
            }
            KeyCode::Char('b') => {
                if let Some(rule) = entry.as_ref().and_then(|e| e.rule.as_ref())
                    && let Err(error) = open_browser(rule.local_port)
                {
                    notice = error.to_string();
                }
            }
            _ if pending.is_some() || changing => {
                notice = "Wait for the current change to finish.".into()
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('r') => {
                if let Some(entry) = &entry
                    && let Some(rule) = &entry.rule
                {
                    pending = Some(Operation {
                        directory: entry.machine.directory.clone(),
                        command: if key.code == KeyCode::Char('r') {
                            "restart"
                        } else if requested(&entry.state) {
                            "stop"
                        } else {
                            "start"
                        },
                        args: json!({"rule_id":rule.id}),
                    });
                }
            }
            KeyCode::Char('a' | 'e') => {
                if let Some(entry) = &entry {
                    let previous = if key.code == KeyCode::Char('e') {
                        entry.rule.clone()
                    } else {
                        None
                    };
                    if key.code == KeyCode::Char('e') && previous.is_none() {
                        continue;
                    }
                    let fields = [
                        (
                            "App port on server",
                            previous
                                .as_ref()
                                .map(|r| r.remote_port.to_string())
                                .unwrap_or_default(),
                        ),
                        (
                            "Port on this computer (blank uses same)",
                            previous
                                .as_ref()
                                .map(|r| r.local_port.to_string())
                                .unwrap_or_default(),
                        ),
                        (
                            "Name (optional)",
                            previous
                                .as_ref()
                                .map(|r| r.name.clone())
                                .unwrap_or_default(),
                        ),
                    ];
                    if let Some(values) = screen::form(
                        &mut session.terminal,
                        if previous.is_some() {
                            " Edit favorite "
                        } else {
                            " Add favorite "
                        },
                        &fields,
                        usize::from(previous.is_some()),
                        &format!(
                            "Server: {} · {}",
                            entry.machine.name,
                            if previous.is_some() {
                                "Save changes"
                            } else {
                                "Save and connect"
                            }
                        ),
                    )? {
                        let result = (|| -> Result<Forward> {
                            let remote = store::port(values[0].trim())?;
                            let local = if values[1].trim().is_empty() {
                                remote
                            } else {
                                store::port(values[1].trim())?
                            };
                            let mut rule = Forward::new(local, remote, &values[2])?;
                            if let Some(previous) = &previous {
                                rule.id = previous.id.clone();
                            }
                            Ok(rule)
                        })();
                        match result {
                            Ok(rule) => {
                                pending = Some(Operation {
                                    directory: entry.machine.directory.clone(),
                                    command: "upsert",
                                    args: json!({"rule":rule,"expected":previous,"start":previous.is_none()}),
                                })
                            }
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
                    pending = Some(Operation {
                        directory: entry.machine.directory.clone(),
                        command: "delete",
                        args: json!({"rule_id":rule.id,"expected":rule}),
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
                    pending = Some(Operation {
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
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}
const HELP: &str = "Quick entry: type 8000 then Enter, or 18000:8000 API. The first number is local; the second is the server's app port. A opens the full form.\n\n↑/↓ select · Enter/Space start or stop · N quick entry\nA add favorite · E edit · D delete · B open local URL\nR reconnect · S stop all listed servers · H manage machines\nF2 return shortcut scope · Q / Ctrl+Q close view\n\nFavorites start OFF. Started forwards retry network failures after 2 seconds, increasing to at most 30 seconds. Enter stops pending retries. Authentication, host-key and occupied-port errors need attention.\n\nBackground is on by default. Closing a tab or the entire terminal leaves the controller running. Signing out or rebooting ends it. --foreground stops the view's connections when it closes.\n\nFor first-time host trust or login problems, run ssh YOUR_ALIAS in a shell. Encrypted keys need ssh-agent. ON confirms an owned local listener, not the health of the remote app.";
