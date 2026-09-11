//! Presentation-only grouping. Keyboard selection always indexes the original entries.
use crate::{
    auto_open::Pending,
    screen::{self, ACCENT, MUTED},
    ui::Entry,
};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Cell, Paragraph, Row, Table},
};
use std::collections::{BTreeMap, VecDeque};

const SELECTED: Color = Color::Rgb(37, 49, 67);

#[derive(Clone, Copy, Debug, PartialEq)]
enum DisplayRow {
    Heading(usize),
    Entry(usize),
}

fn grouped(rows: &[Entry]) -> Vec<DisplayRow> {
    let mut indexes = BTreeMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, entry) in rows.iter().enumerate() {
        let group = *indexes.entry(entry.machine.id.as_str()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(index);
    }
    groups
        .into_iter()
        .flat_map(|group| {
            std::iter::once(DisplayRow::Heading(group[0]))
                .chain(group.into_iter().map(DisplayRow::Entry))
        })
        .collect()
}

/// Repeat context above a clipped group, without ever replacing the selected entry.
fn viewport(rows: &[DisplayRow], selected: usize, height: usize) -> Vec<DisplayRow> {
    if height == 0 || rows.is_empty() {
        return Vec::new();
    }
    let position = rows
        .iter()
        .position(|row| *row == DisplayRow::Entry(selected))
        .unwrap_or(1.min(rows.len() - 1));
    if height == 1 {
        return vec![rows[position]];
    }
    let mut start = position.saturating_sub(height - 1);
    if matches!(rows[start], DisplayRow::Entry(_)) {
        start += 1;
    }
    let mut visible = Vec::with_capacity(height);
    if matches!(rows[start], DisplayRow::Entry(_))
        && let Some(heading) = rows[..start]
            .iter()
            .rev()
            .find(|row| matches!(row, DisplayRow::Heading(_)))
    {
        visible.push(*heading);
    }
    visible.extend(rows[start..].iter().take(height - visible.len()));
    visible
}

fn state<'a>(entry: &'a Entry, pending: &VecDeque<Pending>) -> &'a str {
    if !matches!(entry.state.as_str(), "ON" | "CONNECTING" | "RETRYING")
        && entry.rule.as_ref().is_some_and(|rule| {
            pending
                .iter()
                .any(|item| item.matches(&entry.machine, rule))
        })
    {
        "QUEUED"
    } else {
        &entry.state
    }
}

fn columns(width: u16) -> Vec<Constraint> {
    if width >= 64 {
        vec![
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(4),
            Constraint::Length(15),
        ]
    } else if width >= 42 {
        vec![
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(13),
        ]
    } else if width >= 26 {
        vec![
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(11),
        ]
    } else {
        vec![Constraint::Min(1)]
    }
}

fn compact_state(value: &str) -> &str {
    match value {
        "CONNECTING" => "START",
        "RETRYING" => "RETRY",
        "QUEUED" => "QUEUE",
        "UNKNOWN" => "?",
        other => other,
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    entries: &[Entry],
    selected: usize,
    pending: &VecDeque<Pending>,
) {
    render_named(frame, area, entries, selected, pending, None);
}
pub fn render_named(
    frame: &mut Frame,
    area: Rect,
    entries: &[Entry],
    selected: usize,
    pending: &VecDeque<Pending>,
    names: Option<&crate::service_name::Inspector>,
) {
    let panel = screen::panel(" Saved connections ");
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let labels = if inner.width >= 64 {
        vec!["NAME", "STATE", "AUTO", "LOCAL / DEST"]
    } else if inner.width >= 26 {
        vec!["NAME", "STATE", "LOCAL/DEST"]
    } else {
        vec!["NAME"]
    };
    let widths = columns(inner.width);
    frame.render_widget(
        Table::new(
            [Row::new(labels).style(Style::default().fg(MUTED))],
            widths.clone(),
        ),
        Rect { height: 1, ..inner },
    );
    let visible = viewport(
        &grouped(entries),
        selected,
        inner.height.saturating_sub(1) as usize,
    );
    for (line, item) in visible.iter().enumerate() {
        let row_area = Rect::new(inner.x, inner.y + 1 + line as u16, inner.width, 1);
        match *item {
            DisplayRow::Heading(index) => {
                let machine = &entries[index].machine;
                let label = if machine.name == machine.target {
                    machine.name.clone()
                } else {
                    format!("{} · {}", machine.name, machine.target)
                };
                frame.render_widget(
                    Paragraph::new(label)
                        .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                    row_area,
                );
            }
            DisplayRow::Entry(index) => {
                let entry = &entries[index];
                let status = state(entry, pending);
                let color = match status {
                    "ON" => Color::Green,
                    "CONNECTING" | "RETRYING" | "QUEUED" => Color::Yellow,
                    "ERROR" => Color::Red,
                    _ => MUTED,
                };
                let name = entry.rule.as_ref().map_or_else(
                    || "No saved forwards · A to add".to_owned(),
                    |rule| {
                        if let Some(label) = names.and_then(|names| names.label(entry)) {
                            label.to_owned()
                        } else if rule.name.is_empty() {
                            if rule.is_socks() {
                                "SOCKS5 proxy".into()
                            } else {
                                format!("Port {}", rule.remote_port.unwrap_or_default())
                            }
                        } else {
                            rule.name.clone()
                        }
                    },
                );
                let mapping = entry.rule.as_ref().map_or_else(String::new, |rule| {
                    if rule.is_socks() {
                        format!("{} SOCKS5", rule.local_port)
                    } else if inner.width >= 42 {
                        format!(
                            "{} → {}",
                            rule.local_port,
                            rule.remote_port.unwrap_or_default()
                        )
                    } else {
                        format!(
                            "{}→{}",
                            rule.local_port,
                            rule.remote_port.unwrap_or_default()
                        )
                    }
                });
                let mut cells = vec![Cell::from(format!(
                    "{}{}",
                    if index == selected { "› " } else { "  " },
                    name
                ))];
                if inner.width >= 26 {
                    cells.push(
                        Cell::from(if inner.width >= 42 {
                            status
                        } else {
                            compact_state(status)
                        })
                        .style(Style::default().fg(color)),
                    );
                    if inner.width >= 64 {
                        cells.push(
                            Cell::from(if entry.open_automatically { "on" } else { "" })
                                .style(Style::default().fg(ACCENT)),
                        );
                    }
                    cells.push(Cell::from(mapping));
                }
                let style = if index == selected {
                    Style::default().bg(SELECTED)
                } else {
                    Style::default()
                };
                frame.render_widget(
                    Table::new([Row::new(cells).style(style)], widths.clone()),
                    row_area,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{machines::Machine, screen::Input, store::Forward, ui::Presentation};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
    use std::path::PathBuf;

    fn entry(machine: &str, name: &str) -> Entry {
        Entry {
            machine: Machine {
                id: machine.into(),
                name: format!("Server {machine}"),
                target: format!("alias-{machine}"),
                directory: PathBuf::from(machine),
                ssh_port: None,
                ssh_config: None,
            },
            rule: Some(Forward {
                id: format!("{machine}-{name}"),
                name: name.into(),
                local_port: 18000,
                remote_port: Some(8000),
                kind: crate::store::ForwardKind::Local,
            }),
            state: "ON".into(),
            details: String::new(),
            open_automatically: false,
        }
    }
    fn draw(entries: &[Entry], selected: usize, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), entries, selected, &VecDeque::new()))
            .unwrap();
        terminal.backend().buffer().clone()
    }
    fn lines(buffer: &Buffer) -> Vec<String> {
        (buffer.area.y..buffer.area.bottom())
            .map(|y| {
                (buffer.area.x..buffer.area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }
    fn selected_line(buffer: &Buffer) -> String {
        let highlighted: Vec<_> = lines(buffer)
            .into_iter()
            .filter(|line| line.contains('›'))
            .collect();
        assert_eq!(highlighted.len(), 1, "{highlighted:?}");
        highlighted[0].clone()
    }

    #[test]
    fn headings_are_not_columns_or_selectable_entries() {
        let mut rows = vec![
            entry("one", "API"),
            entry("one", "Dashboard"),
            entry("two", "API"),
        ];
        rows[0].open_automatically = true;
        // Display names can collide; grouping uses the stable machine ID.
        rows[2].machine.name = rows[0].machine.name.clone();
        let buffer = draw(&rows, 2, 86, 13);
        let text = lines(&buffer);
        let header = text.iter().find(|line| line.contains("NAME")).unwrap();
        assert!(header.find("NAME").unwrap() < header.find("STATE").unwrap());
        assert!(!header.contains("SERVER") && !header.contains("FAVORITE"));
        assert_eq!(
            text.iter()
                .filter(|line| line.contains("alias-one"))
                .count(),
            1
        );
        assert_eq!(
            text.iter()
                .filter(|line| line.contains("alias-two"))
                .count(),
            1
        );
        assert!(selected_line(&buffer).contains("API"));
        let selected_y = text.iter().position(|line| line.contains('›')).unwrap();
        assert!(text[selected_y - 1].contains("alias-two"));
        assert_eq!(buffer[(2, selected_y as u16)].bg, SELECTED);
        assert_ne!(buffer[(2, (selected_y - 1) as u16)].bg, SELECTED);
        assert!(text.iter().any(|line| line.contains("AUTO")));
        assert!(
            text.iter()
                .any(|line| line.contains("API") && line.contains("on"))
        );
    }

    #[test]
    fn grouped_projection_preserves_original_entry_indexes() {
        let rows = vec![
            entry("one", "first"),
            entry("two", "second"),
            entry("one", "third"),
        ];
        assert_eq!(
            grouped(&rows),
            vec![
                DisplayRow::Heading(0),
                DisplayRow::Entry(0),
                DisplayRow::Entry(2),
                DisplayRow::Heading(1),
                DisplayRow::Entry(1)
            ]
        );
        for (index, row) in rows.iter().enumerate() {
            assert!(
                selected_line(&draw(&rows, index, 80, 10))
                    .contains(&row.rule.as_ref().unwrap().name)
            );
        }
    }

    #[test]
    fn long_group_scroll_repeats_heading_and_keeps_selected_forward_visible() {
        let rows: Vec<_> = (0..60)
            .map(|i| entry("long", &format!("service-{i:02}")))
            .collect();
        for selected in [0, 1, 8, 20, 59] {
            let buffer = draw(&rows, selected, 72, 8);
            let text = lines(&buffer);
            assert!(text.iter().any(|line| line.contains("Server long")));
            assert!(selected_line(&buffer).contains(&format!("service-{selected:02}")));
        }
    }

    #[test]
    fn many_groups_keep_selected_machine_context_and_empty_machine_target() {
        let mut rows: Vec<_> = (0..40)
            .map(|i| entry(&format!("{i:02}"), &format!("service-{i:02}")))
            .collect();
        rows[27].rule = None;
        for selected in 0..rows.len() {
            let buffer = draw(&rows, selected, 72, 7);
            let text = lines(&buffer);
            assert!(
                text.iter()
                    .any(|line| line.contains(&format!("Server {selected:02}")))
            );
            assert!(selected_line(&buffer).contains(if selected == 27 {
                "No saved forwards"
            } else {
                "service-"
            }));
        }
        assert_eq!(
            viewport(&grouped(&rows), 27, 1),
            vec![DisplayRow::Entry(27)]
        );
    }

    #[test]
    fn narrow_unicode_and_tiny_areas_keep_name_first_without_panicking() {
        let mut rows = vec![entry("測試", "開發 API")];
        rows[0].state = "CONNECTING".into();
        let buffer = draw(&rows, 0, 40, 7);
        let selected = selected_line(&buffer);
        // Wide glyphs occupy a symbol cell plus a blank continuation cell.
        assert_eq!(buffer[(3, 3)].symbol(), "開");
        assert_eq!(buffer[(5, 3)].symbol(), "發");
        assert!(
            selected.contains("API")
                && selected.contains("START")
                && selected.contains("18000→8000"),
            "{selected:?}"
        );
        for (width, height) in [(1, 1), (10, 2), (22, 4), (40, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    crate::ui::render(
                        frame,
                        &Presentation {
                            rows: &rows,
                            selected: 0,
                            quick: &Input::default(),
                            typing: false,
                            notice: "",
                            busy: false,
                            persistent: true,
                            automatic: &VecDeque::new(),
                            automatic_errors: &BTreeMap::new(),
                        },
                    )
                })
                .unwrap();
            if (width, height) == (40, 12) {
                assert!(selected_line(terminal.backend().buffer()).contains("API"));
            }
        }
    }

    #[test]
    fn blank_legacy_name_uses_only_the_existing_port_fallback() {
        let rows = vec![entry("one", "")];
        assert!(selected_line(&draw(&rows, 0, 72, 7)).contains("Port 8000"));
    }

    #[test]
    fn proxy_rows_identify_socks_without_a_fake_remote_port() {
        let mut row = entry("one", "Proxy");
        row.rule = Some(Forward::socks(1080, "Proxy").unwrap());
        let text = selected_line(&draw(&[row], 0, 64, 8));
        assert!(
            text.contains("Proxy") && text.contains("1080 SOCKS5"),
            "{text}"
        );
        assert!(!text.contains('→'));
    }
}
