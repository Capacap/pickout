//! pickout — a terminal file chooser that is its own portal backend.
//!
//! One binary, two modes. The daemon half answers D-Bus and has no terminal, so
//! it spawns one running this same binary in dialog mode. Making both ends the
//! same program is what lets the contract between them be a pair of shared
//! types rather than an undocumented argv convention.

mod config;
mod dialog;
mod field;
mod keys;
mod model;
mod portal;
mod protocol;
mod terminal;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use tokio::signal::unix::{SignalKind, signal};

const USAGE: &str = "\
usage:
  pickout --serve                          run as the portal backend
  pickout --dialog <request> <response>    run the chooser in this terminal
";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pickout=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match dispatch() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--serve") => serve(),
        Some("--dialog") => {
            let [request, response] = args.get(1..3).unwrap_or(&[]) else {
                bail!("--dialog needs a request path and a response path\n\n{USAGE}");
            };
            dialog::run(&PathBuf::from(request), &PathBuf::from(response))
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn serve() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(async {
            tracing::info!("serving org.freedesktop.impl.portal.FileChooser");
            let config = config::Config::load();
            if let Some(problem) = &config.problem {
                tracing::warn!("{problem}");
            }

            ashpd::backend::Builder::new("org.freedesktop.impl.portal.desktop.pickout")?
                .file_chooser(portal::FileChooser::new(&config))
                .build()
                .await?;

            // `build` registers the interfaces and returns; the connection
            // lives on in the background, so the process has to be kept alive
            // by something. systemd stops a Type=dbus unit with SIGTERM, so
            // waiting only on SIGINT would mean being killed rather than
            // shutting down.
            let mut term = signal(SignalKind::terminate())?;
            let mut int = signal(SignalKind::interrupt())?;
            tokio::select! {
                _ = term.recv() => tracing::info!("SIGTERM; shutting down"),
                _ = int.recv() => tracing::info!("SIGINT; shutting down"),
            }
            Ok::<_, anyhow::Error>(())
        })
}
