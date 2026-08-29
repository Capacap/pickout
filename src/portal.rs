//! The `org.freedesktop.impl.portal.FileChooser` backend.
//!
//! ashpd does the D-Bus half: Request objects, the response tuple, and the
//! `ay`-versus-`s` asymmetry of `current_folder`/`current_file`/`current_name`
//! that is so easy to get wrong by hand. What is left is translating portal
//! options into a [`protocol::Request`], running the dialog, and translating
//! back.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ashpd::backend::Result as PortalResult;
use ashpd::backend::file_chooser::FileChooserImpl;
use ashpd::backend::request::RequestImpl;
use ashpd::desktop::HandleToken;
use ashpd::desktop::file_chooser::{
    FileFilter, OpenFileOptions, SaveFileOptions, SaveFilesOptions, SelectedFiles,
};
use ashpd::{MaybeAppID, PortalError, Uri, WindowIdentifierType};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};

use crate::protocol::{self, Mode, Request, Response, WirePath};
use crate::terminal::Terminal;

/// Everything outside the URI unreserved set gets escaped; `/` stays as the
/// separator. Operates on bytes, so paths that are not UTF-8 encode correctly
/// rather than failing.
const PATH_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Where the browser opens, when the application's suggestion is not simply
/// obeyed.
///
/// `Last` deliberately *overrides* the application rather than filling in for
/// it: most GTK applications propose `~/Documents` for every save regardless of
/// what is being saved, so their suggestion is worse than knowing where the
/// previous save actually landed. This mirrors `save_mode`/`open_mode` in the
/// daemon being replaced, whose defaults were reached the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirPolicy {
    Suggested,
    Last,
    Fixed(PathBuf),
}

impl DirPolicy {
    fn from_env(var: &str, fallback: Self) -> Self {
        match std::env::var(var).as_deref() {
            Ok("suggested") => Self::Suggested,
            Ok("last") => Self::Last,
            Ok(path) if path.starts_with('/') => Self::Fixed(PathBuf::from(path)),
            _ => fallback,
        }
    }

    /// `suggested` is the application's `current_folder`; `last` is where the
    /// previous dialog landed. Anything that is not an existing directory is
    /// discarded in favour of `default`, so the dialog is never handed a path
    /// it has to make excuses for.
    fn resolve(&self, suggested: Option<&Path>, last: Option<&Path>, default: &Path) -> PathBuf {
        let candidate = match self {
            Self::Suggested => suggested,
            Self::Last => last,
            Self::Fixed(path) => Some(path.as_path()),
        };
        candidate
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| default.to_path_buf())
    }
}

pub struct FileChooser {
    terminal: Terminal,
    open_dir: DirPolicy,
    save_dir: DirPolicy,
}

impl FileChooser {
    pub fn from_env() -> Self {
        Self {
            terminal: Terminal::from_env(),
            open_dir: DirPolicy::from_env("FILEPICK_OPEN_DIR", DirPolicy::Suggested),
            save_dir: DirPolicy::from_env("FILEPICK_SAVE_DIR", DirPolicy::Last),
        }
    }

    /// Writes the request, runs the dialog, reads the response.
    ///
    /// The request and response files are removed on every path out, including
    /// the one where the future is dropped mid-flight — see [`Scratch`].
    async fn run(&self, token: &HandleToken, request: &Request) -> Result<Response> {
        let scratch = Scratch::new(token)?;
        std::fs::write(&scratch.request, serde_json::to_vec_pretty(request)?)
            .with_context(|| format!("writing {}", scratch.request.display()))?;

        let exe = std::env::current_exe().context("locating our own binary")?;
        let status = self
            .terminal
            .run([
                exe.as_os_str(),
                "--dialog".as_ref(),
                scratch.request.as_os_str(),
                scratch.response.as_os_str(),
            ])
            .await?;

        // A dialog that died without writing anything is a cancel, not an
        // error: the window may simply have been closed. Only say so out loud.
        let raw = std::fs::read(&scratch.response).unwrap_or_default();
        if raw.is_empty() {
            tracing::debug!("dialog exited {status} leaving no response; treating as cancelled");
            return Ok(Response::Cancelled);
        }
        serde_json::from_slice(&raw).context("parsing the dialog's response")
    }

    /// Turns the dialog's answer into a portal reply, and remembers where it
    /// landed for the next [`DirPolicy::Last`] resolution.
    fn finish(&self, response: Response, filters: &[FileFilter]) -> PortalResult<SelectedFiles> {
        let Response::Selected { paths, filter } = response else {
            return Err(PortalError::Cancelled("dialog cancelled".into()));
        };
        if paths.is_empty() {
            return Err(PortalError::Cancelled("nothing selected".into()));
        }

        if let Some(last) = paths.last() {
            remember_last_dir(&last.to_path_buf());
        }

        let mut selected = SelectedFiles::default();
        for path in &paths {
            selected = selected.uri(file_uri(&path.to_path_buf())?);
        }
        if let Some(chosen) = filter.and_then(|i| filters.get(i)) {
            selected = selected.current_filter(chosen.clone());
        }
        Ok(selected)
    }
}

#[async_trait::async_trait]
impl RequestImpl for FileChooser {
    /// ashpd aborts the in-flight handler when an application withdraws its
    /// request, which drops the future and — via `kill_on_drop` — takes the
    /// terminal with it, and drops [`Scratch`] with it too. Nothing left to do
    /// here but say it happened.
    ///
    /// The withdrawn call itself answers the caller with a D-Bus error rather
    /// than response 1: ashpd's `Close` unregisters the Request object, and the
    /// aborted handler then tries to unregister it again and fails. That is
    /// upstream and harmless — whoever called `Close` has stopped waiting for
    /// the reply — but it looks alarming in a bus trace, so: not ours.
    async fn close(&self, token: HandleToken) {
        tracing::debug!("request {token} closed by the application");
    }
}

#[async_trait::async_trait]
impl FileChooserImpl for FileChooser {
    async fn open_file(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window: Option<WindowIdentifierType>,
        title: &str,
        options: OpenFileOptions,
    ) -> PortalResult<SelectedFiles> {
        tracing::debug!("OpenFile from {app_id:?}: {title}");
        let multiple = options.multiple().unwrap_or(false);
        let request = Request {
            mode: if options.directory().unwrap_or(false) {
                Mode::OpenDirectory { multiple }
            } else {
                Mode::OpenFile { multiple }
            },
            title: title.to_owned(),
            accept_label: options.accept_label().map(ToOwned::to_owned),
            start_dir: WirePath::from(self.open_dir.resolve(
                options.current_folder().map(AsRef::as_ref),
                last_dir().as_deref(),
                &default_dir(),
            )),
            current_name: None,
            filters: options.filters().iter().map(convert_filter).collect(),
            current_filter: index_of_filter(options.current_filter(), options.filters()),
        };

        let response = self.run(&token, &request).await.map_err(internal)?;
        self.finish(response, options.filters())
    }

    async fn save_file(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window: Option<WindowIdentifierType>,
        title: &str,
        options: SaveFileOptions,
    ) -> PortalResult<SelectedFiles> {
        tracing::debug!("SaveFile from {app_id:?}: {title}");
        // An application may say where via `current_folder`, or via the parent
        // of a full `current_file`, or not at all.
        let suggested_dir = options
            .current_folder()
            .map(|f| f.as_ref().to_path_buf())
            .or_else(|| {
                options
                    .current_file()
                    .and_then(|f| f.as_ref().parent().map(Path::to_path_buf))
            });
        let suggested_name = options.current_name().map(ToOwned::to_owned).or_else(|| {
            options
                .current_file()
                .and_then(|f| f.as_ref().file_name())
                .map(|n| n.to_string_lossy().into_owned())
        });

        let request = Request {
            mode: Mode::SaveFile,
            title: title.to_owned(),
            accept_label: options.accept_label().map(ToOwned::to_owned),
            start_dir: WirePath::from(self.save_dir.resolve(
                suggested_dir.as_deref(),
                last_dir().as_deref(),
                &default_dir(),
            )),
            current_name: suggested_name,
            filters: options.filters().iter().map(convert_filter).collect(),
            current_filter: index_of_filter(options.current_filter(), options.filters()),
        };

        let response = self.run(&token, &request).await.map_err(internal)?;
        self.finish(response, options.filters())
    }

    async fn save_files(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window: Option<WindowIdentifierType>,
        title: &str,
        options: SaveFilesOptions,
    ) -> PortalResult<SelectedFiles> {
        tracing::debug!("SaveFiles from {app_id:?}: {title}");
        // The application already knows the names; the only open question is
        // which directory they land in.
        let request = Request {
            mode: Mode::SaveFiles {
                names: options.files().iter().map(WirePath::from).collect(),
            },
            title: title.to_owned(),
            accept_label: options.accept_label().map(ToOwned::to_owned),
            start_dir: WirePath::from(self.save_dir.resolve(
                options.current_folder().map(AsRef::as_ref),
                last_dir().as_deref(),
                &default_dir(),
            )),
            current_name: None,
            filters: Vec::new(),
            current_filter: None,
        };

        let response = self.run(&token, &request).await.map_err(internal)?;
        self.finish(response, &[])
    }
}

/// The request/response pair for one dialog, removed when this is dropped.
struct Scratch {
    request: PathBuf,
    response: PathBuf,
}

impl Scratch {
    fn new(token: &HandleToken) -> Result<Self> {
        let dir = runtime_dir();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        // Handle tokens are validated by the portal spec to be alphanumeric
        // plus underscore, so they are safe as a filename component.
        Ok(Self {
            request: dir.join(format!("{token}.request.json")),
            response: dir.join(format!("{token}.response.json")),
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.request);
        let _ = std::fs::remove_file(&self.response);
    }
}

fn convert_filter(filter: &FileFilter) -> protocol::Filter {
    let globs = filter
        .pattern_filters()
        .into_iter()
        .map(|p| protocol::FilterRule::Glob(p.to_owned()));
    let mimes = filter
        .mimetype_filters()
        .into_iter()
        .map(|m| protocol::FilterRule::MimeType(m.to_owned()));
    protocol::Filter {
        label: filter.label().to_owned(),
        rules: globs.chain(mimes).collect(),
    }
}

/// `current_filter` arrives as a whole filter rather than an index, so it has
/// to be located in the list by value to be referred to across the wire.
fn index_of_filter(current: Option<&FileFilter>, filters: &[FileFilter]) -> Option<usize> {
    let current = current?;
    filters.iter().position(|f| f == current)
}

fn file_uri(path: &Path) -> PortalResult<Uri> {
    if !path.is_absolute() {
        return Err(PortalError::Failed(format!(
            "dialog returned a relative path: {}",
            path.display()
        )));
    }
    let encoded = percent_encode(path.as_os_str().as_bytes(), PATH_ESCAPE);
    Uri::parse(&format!("file://{encoded}"))
        .map_err(|err| PortalError::Failed(format!("building a file URI: {err:?}")))
}

fn internal(err: anyhow::Error) -> PortalError {
    tracing::error!("{err:#}");
    PortalError::Failed(format!("{err:#}"))
}

fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("filepick")
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"))
        .join("filepick")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn default_dir() -> PathBuf {
    let home = home();
    if home.is_dir() {
        home
    } else {
        PathBuf::from("/")
    }
}

fn last_dir() -> Option<PathBuf> {
    let raw = std::fs::read(state_dir().join("last_dir")).ok()?;
    let trimmed = raw.strip_suffix(b"\n").unwrap_or(&raw);
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(trimmed)))
}

/// Records the directory a selection landed in.
///
/// The path itself need not exist — in a save it usually does not yet, which is
/// precisely where the old daemon's version of this gave up — so the parent is
/// taken by string surgery rather than by asking the filesystem.
fn remember_last_dir(selected: &Path) {
    let dir = if selected.is_dir() {
        selected
    } else {
        match selected.parent() {
            Some(parent) => parent,
            None => return,
        }
    };

    let state = state_dir();
    if std::fs::create_dir_all(&state).is_err() {
        return;
    }
    let _ = std::fs::write(state.join("last_dir"), dir.as_os_str().as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_wins_when_it_is_a_real_directory() {
        let policy = DirPolicy::Suggested;
        let resolved = policy.resolve(
            Some(Path::new("/usr")),
            Some(Path::new("/etc")),
            Path::new("/"),
        );
        assert_eq!(resolved, Path::new("/usr"));
    }

    #[test]
    fn a_suggestion_that_does_not_exist_falls_back_to_the_default() {
        let policy = DirPolicy::Suggested;
        let resolved = policy.resolve(
            Some(Path::new("/nonexistent-uxyz")),
            Some(Path::new("/etc")),
            Path::new("/usr"),
        );
        assert_eq!(resolved, Path::new("/usr"));
    }

    #[test]
    fn last_overrides_the_application_suggestion() {
        let policy = DirPolicy::Last;
        let resolved = policy.resolve(
            Some(Path::new("/usr")),
            Some(Path::new("/etc")),
            Path::new("/"),
        );
        assert_eq!(resolved, Path::new("/etc"));
    }

    #[test]
    fn spaces_and_specials_are_escaped_in_uris() {
        let uri = file_uri(Path::new("/home/me/a b&c#d.txt")).unwrap();
        assert_eq!(uri.as_str(), "file:///home/me/a%20b%26c%23d.txt");
    }

    #[test]
    fn plain_paths_are_left_alone() {
        let uri = file_uri(Path::new("/home/me/notes-1_v2.txt")).unwrap();
        assert_eq!(uri.as_str(), "file:///home/me/notes-1_v2.txt");
    }

    #[test]
    fn relative_paths_are_refused() {
        assert!(file_uri(Path::new("relative/path")).is_err());
    }
}
