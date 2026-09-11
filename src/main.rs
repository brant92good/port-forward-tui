use anyhow::Result;
use clap::Parser;
use port_forward_tui::{
    background,
    cli::{self, Options},
    machines::Catalog,
    ui, views,
};
use serde_json::json;
use std::time::Duration;
fn run(options: &Options) -> Result<i32> {
    port_forward_tui::process::protect_incoming_stdio()?;
    port_forward_tui::channel::validate_directory(&options.data_dir)?;
    if options.serve {
        background::serve(&options.data_dir)?;
        return Ok(0);
    }
    if options.command.is_some() {
        let result = cli::execute(options)?;
        cli::print(&result, options.json);
        return Ok(if result["ok"] == true { 0 } else { 1 });
    }
    let catalog = Catalog::new(&options.data_dir)?;
    let selector = if let Some(host) = &options.host {
        Some(catalog.add(host, "", None, None)?.id)
    } else {
        options.machine.clone()
    };
    if options.check {
        let machines = if let Some(selector) = &selector {
            vec![catalog.get(selector)?]
        } else {
            catalog.list()?
        };
        println!(
            "OK: {} machines; installation needs no host until first use.",
            machines.len()
        );
        return Ok(0);
    }
    if options.stop_all || options.stop_daemon {
        let directory = if selector.is_none() && catalog.directory.join("endpoint.json").exists() {
            catalog.directory.clone()
        } else {
            cli::selected_machine(&catalog, selector.as_deref())?
                .ok_or_else(|| anyhow::anyhow!("Choose a machine with --machine."))?
                .directory
        };
        background::exchange(
            &directory,
            if options.stop_daemon {
                "shutdown"
            } else {
                "stop_all"
            },
            json!({}),
            Duration::from_secs(5),
        )?;
        println!("Selected machine's tunnels stopped.");
        return Ok(0);
    }
    let saved = catalog.list()?;
    let machine = if selector.is_none()
        && !options.machines
        && !options.focus_existing
        && !saved.is_empty()
    {
        Some(saved[0].clone())
    } else {
        ui::choose_machine(&catalog, selector.as_deref(), options.machines, true)?
    };
    if let Some(machine) = machine {
        if options.focus_existing && !options.foreground && cfg!(windows) {
            let origin = views::mark_origin()?;
            let scope = views::read_scope(&machine.directory)?;
            if views::try_focus(&machine.directory.join("views"), &scope, &origin, false)? {
                return Ok(0);
            }
        }
        ui::run(catalog, machine, options.foreground)?;
    }
    Ok(0)
}
fn main() {
    let as_json = std::env::args_os().any(|arg| arg == "--json");
    let options = match Options::try_parse() {
        Ok(options) => options,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return;
            }
            cli::print(
                &json!({"schema_version":2,"channel":"beta","version":env!("CARGO_PKG_VERSION"),"ok":false,"error":{"code":"invalid_arguments","message":error.to_string()}}),
                as_json,
            );
            std::process::exit(2);
        }
    };
    let code = match run(&options) {
        Ok(code) => code,
        Err(error) => {
            let usage = error.is::<cli::UsageError>();
            cli::print(
                &json!({"schema_version":2,"channel":"beta","version":env!("CARGO_PKG_VERSION"),"ok":false,"error":{"code":if usage{"invalid_arguments"}else{"operation_failed"},"message":format!("{error:#}")}}),
                as_json,
            );
            if usage { 2 } else { 1 }
        }
    };
    std::process::exit(code);
}
