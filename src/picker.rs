use crate::{
    machines::{self, Catalog, Machine},
    screen::{self, ACCENT, Input, MUTED, Screen},
    store,
};
use anyhow::Result;
use crossterm::event::{Event, KeyCode};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    widgets::{List, ListItem, ListState, Paragraph},
};
use std::{collections::HashSet, path::PathBuf};

pub fn choose(screen: &mut Screen, catalog: &Catalog) -> Result<Option<Machine>> {
    let mut selected = 0;
    let mut search = Input::default();
    let mut searching = false;
    let mut notice = String::new();
    loop {
        let machines = catalog.list()?;
        let query = search.value.to_lowercase();
        let filtered = machines
            .iter()
            .filter(|m| {
                format!("{} {}", m.name, m.target)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect::<Vec<_>>();
        selected = selected.min(filtered.len().saturating_sub(1));
        screen.draw(|frame|{
            let rows=Layout::vertical([Constraint::Length(3),Constraint::Length(3),Constraint::Min(4),Constraint::Length(3)]).margin(1).split(frame.area());
            frame.render_widget(Paragraph::new("Choose a machine for Ports").block(screen::panel(" PORTS ")),rows[0]);
            frame.render_widget(Paragraph::new(if searching{format!("Search > {}",search.value)}else{format!("{} machines · / searches",machines.len())}).style(Style::default().fg(ACCENT)),rows[1]);
            let items=filtered.iter().map(|m|ListItem::new(format!("{}  ·  {}",m.name,m.target))).collect::<Vec<_>>();
            frame.render_stateful_widget(List::new(items).block(screen::panel(" Machines ")).highlight_symbol("› ").highlight_style(Style::default().fg(ACCENT)),rows[2],&mut ListState::default().with_selected(Some(selected)));
            frame.render_widget(Paragraph::new(format!("↑↓ select · Enter opens · A add · I import SSH · / search · Esc cancel\n{notice}")).style(Style::default().fg(MUTED)),rows[3]);
        })?;
        match screen::key()? {
            Some(Event::Key(key)) if screen::quit(key) => return Ok(None),
            Some(Event::Key(key)) if searching => match key.code {
                KeyCode::Esc | KeyCode::Enter => searching = false,
                KeyCode::Down => selected = (selected + 1).min(filtered.len().saturating_sub(1)),
                KeyCode::Up => selected = selected.saturating_sub(1),
                _ => {
                    search.key(key);
                    selected = 0;
                }
            },
            Some(Event::Key(key)) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(filtered.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some(machine) = filtered.get(selected) {
                        return Ok(Some((*machine).clone()));
                    }
                }
                KeyCode::Char('/') => searching = true,
                KeyCode::Char('a') => {
                    if let Some(values) = screen::form(
                        screen,
                        " Add a machine ",
                        &[
                            ("SSH alias or user@address", String::new()),
                            ("Name (optional)", String::new()),
                            ("SSH login port (optional)", String::new()),
                            ("SSH config path (optional)", String::new()),
                        ],
                        0,
                        "App ports are added after choosing the machine.",
                    )? {
                        let result = (|| -> Result<Machine> {
                            let port = if values[2].trim().is_empty() {
                                None
                            } else {
                                Some(store::port(values[2].trim())?)
                            };
                            let path = if values[3].trim().is_empty() {
                                None
                            } else {
                                Some(PathBuf::from(values[3].trim()))
                            };
                            catalog.add(&values[0], &values[1], port, path.as_deref())
                        })();
                        match result {
                            Ok(machine) => return Ok(Some(machine)),
                            Err(error) => notice = format!("{error:#}"),
                        }
                    }
                }
                KeyCode::Char('i') => match import(screen, catalog) {
                    Ok(Some(machine)) => return Ok(Some(machine)),
                    Ok(None) => {}
                    Err(error) => notice = format!("{error:#}"),
                },
                _ => {}
            },
            Some(Event::Paste(text)) if searching => search.insert(&text),
            _ => {}
        }
    }
}
fn import(screen: &mut Screen, catalog: &Catalog) -> Result<Option<Machine>> {
    let Some(values) = screen::form(
        screen,
        " Import SSH hosts ",
        &[(
            "SSH config file",
            machines::default_ssh_config().display().to_string(),
        )],
        0,
        "Read host names first, then choose which ones to import.",
    )?
    else {
        return Ok(None);
    };
    let path = PathBuf::from(values[0].trim());
    let aliases = machines::ssh_aliases(Some(&path))?;
    if aliases.is_empty() {
        screen::message(
            screen,
            " No hosts found ",
            "No literal Host names were found. You can add a machine manually.",
            false,
        )?;
        return Ok(None);
    }
    let mut checked = HashSet::new();
    let mut selected = 0;
    loop {
        screen.draw(|frame|{let areas=Layout::vertical([Constraint::Min(4),Constraint::Length(3)]).margin(1).split(frame.area());let items=aliases.iter().map(|alias|ListItem::new(format!("[{}] {alias}",if checked.contains(alias){"x"}else{" "}))).collect::<Vec<_>>();
            frame.render_stateful_widget(List::new(items).block(screen::panel(" Import SSH hosts ")).highlight_symbol("› ").highlight_style(Style::default().fg(ACCENT)),areas[0],&mut ListState::default().with_selected(Some(selected)));
            frame.render_widget(Paragraph::new("Space selects · A selects all · Enter imports selected (or highlighted) · Esc cancels").style(Style::default().fg(MUTED)),areas[1]);})?;
        if let Some(Event::Key(key)) = screen::key()? {
            if screen::quit(key) {
                return Ok(None);
            }
            match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(aliases.len() - 1),
                KeyCode::Char(' ') => {
                    let alias = aliases[selected].clone();
                    if !checked.remove(&alias) {
                        checked.insert(alias);
                    }
                }
                KeyCode::Char('a') => {
                    checked = aliases.iter().cloned().collect();
                }
                KeyCode::Enter => {
                    if checked.is_empty() {
                        checked.insert(aliases[selected].clone());
                    }
                    let chosen = aliases
                        .iter()
                        .filter(|alias| checked.contains(*alias))
                        .cloned()
                        .collect::<Vec<_>>();
                    return Ok(machines::import(catalog, Some(&path), Some(&chosen))?
                        .into_iter()
                        .next());
                }
                _ => {}
            }
        }
    }
}
