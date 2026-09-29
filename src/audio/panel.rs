use super::*;
use crate::screen;
use crossterm::event::{Event, KeyCode};
use ratatui::{
    layout::Margin,
    widgets::{Clear, Paragraph, Wrap},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub fn show(terminal: &mut screen::Screen, machine: &Machine, root: &Path) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    let mut working = false;
    let cancel_start = Arc::new(AtomicBool::new(false));
    let mut notice = String::new();
    let mut next = Instant::now();
    let mut current = status(machine)?;
    loop {
        if let Ok((was_start, result)) = receiver.try_recv() {
            if was_start && cancel_start.load(Ordering::Relaxed) {
                let (machine, sender) = (machine.clone(), sender.clone());
                thread::spawn(move || {
                    let text = stop(&machine)
                        .map(|v| {
                            format!("Capture stopped. Restoration: {}", v["last"]["restoration"])
                        })
                        .unwrap_or_else(|e| format!("Stop did not complete: {e:#}"));
                    let _ = sender.send((false, text));
                });
            } else {
                working = false;
            }
            notice = result;
            next = Instant::now();
        }
        if Instant::now() >= next {
            current = status(machine)?;
            next = Instant::now() + Duration::from_millis(500);
        }
        terminal.draw(|frame| {
            let area = screen::centered(frame.area(), 86, 22);
            frame.render_widget(Clear, area);
            frame.render_widget(screen::panel(" Audio forwarding · Experimental "), area);
            let phase = if current["running"] == true { current["last"]["phase"].as_str().unwrap_or("starting") } else { "OFF" };
            let setup = if current["supported"] != true { "Experimental audio needs a Windows client and a prepared Linux bridge.\nOn this platform microphone capture is unavailable." }
                else if current["enabled"] == true { "Enter starts microphone + reply playback. S stops.\nUse Stop before closing; a terminal host may end background audio." }
                else { "Audio is disabled by default. Run ports-beta audio configure --help\nto select your microphone and prepared Linux bridge." };
            let playback = if current["supported"] == true { "Windows default output (use headphones)" } else { "unavailable on this client" };
            let controls = if current["supported"] == true { "Enter start · S stop · Esc back" } else { "Esc back" };
            let body = format!("{}\n\nState: {phase}\nMicrophone: {}\nPlayback: {playback}\n\n{setup}\nNo automatic recording or reconnect. No audio files are saved.\n\nRestoration: {}\n{}\n\n{}\n\n{controls}", machine.name,
                current["microphone"].as_str().unwrap_or("not configured"),
                current["last"]["restoration"].as_str().unwrap_or("not needed"),
                current["configuration_error"].as_str().or(current["last"]["error"].as_str()).unwrap_or(""),
                if working && cancel_start.load(Ordering::Relaxed) { "Stop queued; finishing startup then stopping. Please wait." }
                else if working { "Working… S can queue Stop while starting." } else { &notice });
            frame.render_widget(Paragraph::new(body).wrap(Wrap { trim:false }), area.inner(Margin::new(2,1)));
        })?;
        let Some(Event::Key(key)) = screen::key()? else {
            continue;
        };
        if !working && (key.code == KeyCode::Esc || screen::quit(key)) {
            return Ok(());
        }
        match key.code {
            KeyCode::Enter
                if !working && current["supported"] == true && current["enabled"] == true =>
            {
                working = true;
                cancel_start.store(false, Ordering::Relaxed);
                let cancel = cancel_start.clone();
                let (machine, root, sender) = (machine.clone(), root.to_path_buf(), sender.clone());
                thread::spawn(move || {
                    if cancel.load(Ordering::Relaxed) {
                        let _ = sender
                            .send((true, "Start cancelled; no microphone opened.".to_string()));
                        return;
                    }
                    let text = start(&machine, &root)
                        .map(|v| {
                            if v["last"]["phase"] == "streaming" {
                                "Transport ready. Select the forwarded audio in your remote app."
                                    .to_string()
                            } else {
                                format!(
                                    "Audio state: {}. Wait before restarting.",
                                    v["last"]["phase"]
                                )
                            }
                        })
                        .unwrap_or_else(|e| format!("{e:#}"));
                    let _ = sender.send((true, text));
                });
            }
            KeyCode::Char('s') if working => {
                cancel_start.store(true, Ordering::Relaxed);
                notice = "Stop queued; finishing startup and then stopping capture.".into();
            }
            KeyCode::Char('s') => {
                let (machine, sender) = (machine.clone(), sender.clone());
                working = true;
                thread::spawn(move || {
                    let text = stop(&machine)
                        .map(|v| {
                            format!("Capture stopped. Restoration: {}", v["last"]["restoration"])
                        })
                        .unwrap_or_else(|e| format!("{e:#}"));
                    let _ = sender.send((false, text));
                });
            }
            _ => {}
        }
    }
}
