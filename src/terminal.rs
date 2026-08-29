//! Spawning the terminal the dialog runs in.
//!
//! The daemon has no terminal of its own, so it has to open one. Two properties
//! are required of whatever gets used, and only the second is a real filter:
//!
//! * it must set the Wayland app-id, because the compositor's float rule
//!   matches on it and nothing warns you when it does not match;
//! * it must **block until the command it was given exits**. A terminal that
//!   hands the command to an already-running instance and returns immediately
//!   makes the daemon read an empty response file and report a cancel, every
//!   time. `kitty` is safe by default; `kitty --single-instance` and
//!   `footclient` are not.
//!
//! Nothing here is specific to kitty beyond the default; `FILEPICK_TERMINAL`
//! replaces the whole invocation.

use std::ffi::{OsStr, OsString};
use std::process::ExitStatus;

use anyhow::{Context, Result};
use tokio::process::Command;

/// Matches the `window-rule` in niri's config.
pub const APP_ID: &str = "filepick";

#[derive(Debug, Clone)]
pub struct Terminal {
    argv: Vec<OsString>,
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            argv: vec![
                "kitty".into(),
                format!("--app-id={APP_ID}").into(),
                // Everything after this is the command, not more kitty options.
                "--".into(),
            ],
        }
    }
}

impl Terminal {
    /// `FILEPICK_TERMINAL` overrides the invocation, split on whitespace.
    ///
    /// No quoting: a terminal invocation with a space inside a single argument
    /// is not a thing that happens, and the escaping dance the shell wrapper
    /// needed here is exactly what made filenames hazardous. Our own arguments
    /// are passed as argv and never go near a shell.
    pub fn from_env() -> Self {
        match std::env::var("FILEPICK_TERMINAL") {
            Ok(spec) if !spec.trim().is_empty() => Self {
                argv: spec.split_whitespace().map(OsString::from).collect(),
            },
            _ => Self::default(),
        }
    }

    /// Runs `command` in the terminal and waits for it to exit.
    ///
    /// The child is killed if this future is dropped, which is what happens
    /// when an application withdraws its request: ashpd aborts the handler, and
    /// an abandoned dialog must not outlive it as an orphan window.
    pub async fn run<I, S>(&self, command: I) -> Result<ExitStatus>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let (program, options) = self
            .argv
            .split_first()
            .context("terminal invocation is empty")?;

        let mut cmd = Command::new(program);
        cmd.args(options);
        cmd.args(command);
        cmd.kill_on_drop(true);

        cmd.status()
            .await
            .with_context(|| format!("failed to run {}", program.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_kitty_with_the_app_id_the_float_rule_matches() {
        let argv = Terminal::default().argv;
        assert_eq!(argv[0], OsStr::new("kitty"));
        assert!(argv.iter().any(|a| a == OsStr::new("--app-id=filepick")));
        assert_eq!(argv.last().unwrap(), OsStr::new("--"));
    }
}
