//! The user's settings.
//!
//! Both halves read the same file and take the part they use: the daemon wants
//! the terminal and the directory policies, the dialog wants the keymap. This is
//! read from disk rather than passed through [`crate::protocol`] because it
//! belongs to the user, not to the application's request — and because the
//! daemon is D-Bus activated, which is the whole reason a file is needed. An
//! activated service inherits the environment its manager was started with, so
//! a variable exported in a shell reaches it only by coincidence.
//!
//! Nothing here is ever fatal. A dialog that fails to start is indistinguishable
//! from a cancel at the far end of the portal, so a file that cannot be read
//! falls back to the defaults and reports itself instead: to the journal on the
//! daemon side, to the message line on the dialog side.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Terminal invocation, split on whitespace. See [`crate::terminal`].
    pub terminal: Option<String>,
    /// `suggested`, `last`, or an absolute path. See [`crate::portal::DirPolicy`].
    pub open_dir: Option<String>,
    pub save_dir: Option<String>,

    /// Why the file was ignored, if it was. Held rather than logged so that the
    /// dialog can put it on screen, which is the only place its user is looking.
    #[serde(skip)]
    pub problem: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        let mut config = Self::read(&path());
        config.apply_env();
        config
    }

    fn read(path: &std::path::Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            // Having no config file is the ordinary case, not a problem.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => return Self::ignored(path, &err.to_string()),
        };
        match toml::from_str::<Self>(&raw) {
            Ok(config) => config,
            // A toml error renders as several lines with a snippet of the
            // source, which is more than a one-line message can hold.
            Err(err) => Self::ignored(path, err.message()),
        }
    }

    /// The environment wins over the file. Applied here rather than at each
    /// point of use so that everything downstream reads one settled value, and
    /// so that the precedence is stated once instead of once per setting.
    fn apply_env(&mut self) {
        for (var, field) in [
            ("FILEPICK_TERMINAL", &mut self.terminal),
            ("FILEPICK_OPEN_DIR", &mut self.open_dir),
            ("FILEPICK_SAVE_DIR", &mut self.save_dir),
        ] {
            if let Ok(value) = std::env::var(var)
                && !value.trim().is_empty()
            {
                *field = Some(value);
            }
        }
    }

    fn ignored(path: &std::path::Path, why: &str) -> Self {
        Self {
            problem: Some(format!("ignoring {}: {}", path.display(), why.trim())),
            ..Self::default()
        }
    }
}

/// `FILEPICK_CONFIG` names the file outright, which is what the tests use to
/// keep the developer's own settings out of them.
fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("FILEPICK_CONFIG") {
        return PathBuf::from(path);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("filepick/config.toml")
}

/// The anchor for every path that belongs to the user rather than to the
/// system. `/` is not a home, but it is somewhere to fail from harmlessly.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        Config::read(&path)
    }

    #[test]
    fn a_missing_file_is_not_a_problem() {
        let config = Config::read(std::path::Path::new("/nonexistent/filepick/config.toml"));
        assert!(config.problem.is_none());
        assert!(config.terminal.is_none());
    }

    #[test]
    fn settings_are_read() {
        let config = read("terminal = \"foot -a filepick\"\nsave_dir = \"/tmp\"\n");
        assert_eq!(config.terminal.as_deref(), Some("foot -a filepick"));
        assert_eq!(config.save_dir.as_deref(), Some("/tmp"));
        assert!(config.problem.is_none());
    }

    #[test]
    fn a_broken_file_reports_itself_and_changes_nothing() {
        let config = read("terminal = \n");
        assert!(config.problem.is_some());
        assert!(config.terminal.is_none());
    }

    #[test]
    fn a_misspelt_key_is_reported_rather_than_ignored() {
        // Silently dropping it is the failure mode that wastes an afternoon.
        let config = read("terminl = \"foot\"\n");
        assert!(config.problem.unwrap().contains("terminl"));
    }
}
