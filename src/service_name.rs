//! Explicit, bounded HTTP title inspection. Never writes or starts a forward.
use crate::{
    background, screen,
    store::{Forward, Store},
    ui::Entry,
};
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    widgets::{Clear, Paragraph, Wrap},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const BODY_LIMIT: u64 = 64 * 1024;
const HEADER_LIMIT: usize = 16 * 1024;
const HTTP_DEADLINE: Duration = Duration::from_millis(1500);
const MAX_LABELS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Frozen {
    machine: String,
    directory: PathBuf,
    host: String,
    ssh_port: Option<u16>,
    ssh_config: Option<String>,
    rule: Forward,
}
impl Frozen {
    fn from_entry(entry: &Entry) -> Result<Self> {
        ensure!(
            entry.state == "ON",
            "Start this forward before checking its web app name."
        );
        let rule = entry
            .rule
            .clone()
            .context("Choose a saved forward first.")?;
        Ok(Self {
            machine: entry.machine.id.clone(),
            directory: entry.machine.directory.clone(),
            host: entry.machine.target.clone(),
            ssh_port: entry.machine.ssh_port,
            ssh_config: entry.machine.ssh_config.clone(),
            rule,
        })
    }
    fn matches(&self, entry: &Entry) -> bool {
        Self::from_entry(entry).is_ok_and(|value| value == *self)
    }
    fn current(&self, persistent: bool) -> Result<()> {
        let store = Store::load(&self.directory)?;
        ensure!(
            store.settings.host == self.host
                && store.settings.ssh_port == self.ssh_port
                && store.settings.ssh_config == self.ssh_config
                && store.settings.forwards.contains(&self.rule),
            "This forward changed. Check it again."
        );
        if persistent {
            let snapshot = background::exchange(
                &self.directory,
                "status",
                json!({}),
                Duration::from_millis(250),
            )?;
            ensure!(
                snapshot["states"][&self.rule.id] == "ON"
                    && snapshot["host"] == self.host
                    && snapshot["ssh_port"] == json!(self.ssh_port)
                    && snapshot["ssh_config"] == json!(self.ssh_config)
                    && snapshot["forwards"]
                        .as_array()
                        .is_some_and(|rows| rows.contains(&json!(self.rule))),
                "This forward is no longer ON with the same mapping."
            );
        }
        Ok(())
    }
    fn key(&self) -> (String, PathBuf, String) {
        (
            self.machine.clone(),
            self.directory.clone(),
            self.rule.id.clone(),
        )
    }
}

/// Reads only the explicitly selected IPv4 loopback port. The fixed URL and
/// disabled proxy/redirect features cannot expand this into network discovery.
fn probe(port: u16, cancelled: &AtomicBool) -> Result<String> {
    ensure!(!cancelled.load(Ordering::Acquire), "Check cancelled.");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .max_response_header_size(HEADER_LIMIT)
        .input_buffer_size(HEADER_LIMIT)
        .output_buffer_size(HEADER_LIMIT)
        .max_idle_connections(0)
        .timeout_global(Some(HTTP_DEADLINE))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Accept", "text/html")
        .header("Accept-Encoding", "identity")
        .header("Connection", "close")
        .header("User-Agent", "Ports-title-check")
        .call()
        .context("Could not read a local HTTP page")?;
    ensure!(
        response.status().as_u16() == 200,
        "No app title: HTTP {} (redirects and login prompts are not followed).",
        response.status().as_u16()
    );
    ensure!(
        response
            .headers()
            .get("content-encoding")
            .is_none_or(|value| value.as_bytes().eq_ignore_ascii_case(b"identity")),
        "Compressed pages are not inspected."
    );
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    ensure!(
        content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/html")),
        "This response is not an HTML page."
    );
    for parameter in content_type.split(';').skip(1) {
        if let Some((name, value)) = parameter.trim().split_once('=')
            && name.trim().eq_ignore_ascii_case("charset")
        {
            ensure!(
                matches!(
                    value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                    "utf-8" | "utf8" | "us-ascii"
                ),
                "This page uses an unsupported text encoding."
            );
        }
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_vec()
        .context("Page exceeded the size or time limit")?;
    ensure!(!cancelled.load(Ordering::Acquire), "Check cancelled.");
    title(std::str::from_utf8(&bytes).context("The page is not valid UTF-8")?)
}

fn title(html: &str) -> Result<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("html > head > title").expect("Static selector");
    let element = document
        .select(&selector)
        .next()
        .context("No HTML page title found. Saved name kept.")?;
    let text = element.text().collect::<String>();
    ensure!(!text.chars().any(|c| (c.is_control() && !matches!(c, '\n' | '\r' | '\t')) || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{206f}')), "The page title contains unsupported control text.");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        unicode_width::UnicodeWidthStr::width(normalized.as_str()) > 0
            && normalized.chars().count() <= 80,
        "The page title must contain 1–80 readable characters."
    );
    Ok(normalized)
}

struct Job {
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<String>>,
}
enum State {
    Reading,
    Ready(String),
    Failed(String),
}
struct Preview {
    frozen: Frozen,
    state: State,
}
struct Label {
    frozen: Frozen,
    title: String,
}

#[derive(Default)]
pub struct Inspector {
    preview: Option<Preview>,
    job: Option<Job>,
    labels: BTreeMap<(String, PathBuf, String), Label>,
}
impl Inspector {
    /// Render the normal preview widget from supplied example HTML. No worker,
    /// connection, controller request or view-label mutation is created.
    #[cfg(feature = "screenshots")]
    pub fn screenshot(entry: &Entry, html: &str) -> Result<Self> {
        Ok(Self {
            preview: Some(Preview {
                frozen: Frozen::from_entry(entry)?,
                state: State::Ready(title(html)?),
            }),
            job: None,
            labels: BTreeMap::new(),
        })
    }
    pub fn open(&mut self, entry: &Entry, persistent: bool) -> Result<()> {
        ensure!(
            self.job.is_none(),
            "The previous check is finishing; try again shortly."
        );
        let frozen = Frozen::from_entry(entry)?;
        let worker_frozen = frozen.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let (send, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = (|| {
                ensure!(!worker_cancel.load(Ordering::Acquire), "Check cancelled.");
                worker_frozen.current(persistent)?;
                probe(worker_frozen.rule.local_port, &worker_cancel)
            })();
            let _ = send.send(result);
        });
        self.job = Some(Job {
            cancelled,
            receiver,
        });
        self.preview = Some(Preview {
            frozen,
            state: State::Reading,
        });
        Ok(())
    }
    pub fn poll(&mut self, entries: &[Entry]) {
        self.labels
            .retain(|_, label| entries.iter().any(|entry| label.frozen.matches(entry)));
        if self
            .preview
            .as_ref()
            .is_some_and(|preview| !entries.iter().any(|entry| preview.frozen.matches(entry)))
        {
            if let Some(job) = &self.job {
                job.cancelled.store(true, Ordering::Release);
            }
            if let Some(preview) = &mut self.preview {
                preview.state =
                    State::Failed("This forward changed or stopped. Check it again.".into());
            }
        }
        let result = self
            .job
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(anyhow::anyhow!("Web title worker stopped.")))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = result {
            let job = self.job.take().unwrap();
            if !job.cancelled.load(Ordering::Acquire)
                && let Some(preview) = &mut self.preview
            {
                preview.state = match result {
                    Ok(title) => State::Ready(title),
                    Err(error) => State::Failed(format!("{error:#}")),
                };
            }
        }
    }
    pub fn close(&mut self) {
        if let Some(job) = &self.job {
            job.cancelled.store(true, Ordering::Release);
        }
        self.preview = None;
    }
    pub fn reset(&mut self, entry: &Entry) {
        if let Some(rule) = &entry.rule {
            self.labels.remove(&(
                entry.machine.id.clone(),
                entry.machine.directory.clone(),
                rule.id.clone(),
            ));
        }
    }
    pub fn label<'a>(&'a self, entry: &Entry) -> Option<&'a str> {
        let rule = entry.rule.as_ref()?;
        self.labels
            .get(&(
                entry.machine.id.clone(),
                entry.machine.directory.clone(),
                rule.id.clone(),
            ))
            .filter(|label| label.frozen.matches(entry))
            .map(|label| label.title.as_str())
    }
    /// Returns true while the dialog owns input. Enter keeps the saved label;
    /// only U explicitly adopts the title, and only after a completed check.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };
        if key.kind != KeyEventKind::Press {
            return true;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close(),
            KeyCode::Enter => {
                self.labels.remove(&preview.frozen.key());
                self.close();
            }
            KeyCode::Char('u') => {
                if let State::Ready(title) = &preview.state {
                    if self.labels.len() >= MAX_LABELS {
                        self.labels.pop_first();
                    }
                    self.labels.insert(
                        preview.frozen.key(),
                        Label {
                            frozen: preview.frozen.clone(),
                            title: title.clone(),
                        },
                    );
                    self.close();
                }
            }
            _ => {}
        }
        true
    }
    pub fn draw(&self, frame: &mut Frame) {
        let Some(preview) = &self.preview else {
            return;
        };
        let area = screen::centered(frame.area(), 76, 14);
        frame.render_widget(Clear, area);
        let block = screen::panel(" Web app name ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let status = match &preview.state {
            State::Reading => "Reading one HTTP page (1.5-second HTTP limit)...".into(),
            State::Ready(title) => format!(
                "Web page title: {title}\n\nEnter: keep saved name    U: use web title in this view"
            ),
            State::Failed(error) => format!("{error}\n\nSaved name kept. Enter or Esc returns."),
        };
        frame.render_widget(Paragraph::new(format!("Saved name: {}\nSource: http://127.0.0.1:{}/ (HTML page title)\n\n{status}\n\nThis display choice is not saved. E restores the saved name.\nEsc cancels. No favorite or forward is changed.", preview.frozen.rule.name, preview.frozen.rule.local_port)).wrap(Wrap { trim:false }), inner);
    }
}
impl Drop for Inspector {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
