//! Render the actual native widgets with example data; no network or user files.
use anyhow::Result;
use port_forward_tui::{
    machines::Machine,
    screen::{self, Input},
    store::Forward,
    ui::{self, Entry, Presentation},
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::{fmt::Write as _, path::Path};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn color(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Green => "#80d9a3".into(),
        Color::Yellow => "#e8c477".into(),
        Color::Red => "#ee8d91".into(),
        Color::White => "#eff2f7".into(),
        _ => fallback.into(),
    }
}
fn save(terminal: &Terminal<TestBackend>, path: &Path, title: &str) -> Result<()> {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width * 9 + 32;
    let height = buffer.area.height * 18 + 48;
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img"><title>{}</title><rect width="100%" height="100%" rx="12" fill="#111620"/><text x="20" y="24" fill="#99a4b6" font-family="monospace" font-size="12">{}</text><g font-family="'Cascadia Mono','DejaVu Sans Mono',monospace" font-size="15" xml:space="preserve">"##,
        escape(title),
        escape(title)
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let px = x * 9 + 16;
            let py = y * 18 + 36;
            let bg = color(cell.bg, "#111620");
            if bg != "#111620" {
                write!(
                    svg,
                    r##"<rect x="{px}" y="{py}" width="9" height="18" fill="{bg}"/>"##
                )?;
            }
            if cell.symbol() != " " && !cell.symbol().is_empty() {
                write!(
                    svg,
                    r##"<text x="{px}" y="{}" fill="{}">{}</text>"##,
                    py + 14,
                    color(cell.fg, "#eff2f7"),
                    escape(cell.symbol())
                )?;
            }
        }
    }
    svg.push_str("</g></svg>\n");
    std::fs::write(path, svg)?;
    Ok(())
}
fn machine(id: &str, name: &str) -> Machine {
    Machine {
        id: id.into(),
        name: name.into(),
        target: id.into(),
        directory: id.into(),
        ssh_port: None,
        ssh_config: None,
    }
}
fn main() -> Result<()> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
    std::fs::create_dir_all(&out)?;
    let development = machine("workbox", "Development");
    let lab = machine("lab", "Lab");
    let rows = [
        (development.clone(), 8000, 8000, "API", "ON"),
        (development.clone(), 3000, 3000, "Project preview", "OFF"),
        (lab.clone(), 18888, 8888, "Jupyter notebook", "ON"),
        (lab, 28000, 8000, "Training API", "RETRYING"),
    ]
    .into_iter()
    .map(|(machine, l, r, n, state)| {
        Ok(Entry {
            machine,
            rule: Some(Forward::new(l, r, n)?),
            state: state.into(),
            details: String::new(),
            open_automatically: false,
        })
    })
    .collect::<Result<Vec<_>>>()?;
    let mut terminal = Terminal::new(TestBackend::new(112, 26))?;
    let quick = Input::default();
    terminal.draw(|frame| {
        ui::render(
            frame,
            &Presentation {
                rows: &rows,
                selected: 0,
                quick: &quick,
                typing: false,
                notice: "",
                busy: false,
                persistent: true,
                automatic: &Default::default(),
                automatic_errors: &Default::default(),
            },
        )
    })?;
    save(
        &terminal,
        &out.join("connections.svg"),
        "Ports · native interface · example data",
    )?;
    let quick = Input::new("18000:8000 API");
    terminal.draw(|frame| {
        ui::render(
            frame,
            &Presentation {
                rows: &rows,
                selected: 0,
                quick: &quick,
                typing: true,
                notice: "",
                busy: false,
                persistent: true,
                automatic: &Default::default(),
                automatic_errors: &Default::default(),
            },
        )
    })?;
    save(
        &terminal,
        &out.join("quick-forward.svg"),
        "N · Quick entry · example data",
    )?;
    let fields = [
        ("App port on server", "8000".into()),
        ("Port on this computer (blank uses same)", "18000".into()),
        ("Name (optional)", "API".into()),
    ];
    let inputs = fields
        .iter()
        .map(|(_, v)| Input::new(v))
        .collect::<Vec<_>>();
    let mut form_terminal = Terminal::new(TestBackend::new(90, 20))?;
    form_terminal.draw(|frame| {
        screen::render_form(
            frame,
            " Add favorite ",
            &fields,
            &inputs,
            1,
            "Server: Development · Save and connect",
        )
    })?;
    save(
        &form_terminal,
        &out.join("add-connection.svg"),
        "A · Add favorite form · example data",
    )?;
    println!("Rendered native list, quick entry and add form with example data.");
    Ok(())
}
