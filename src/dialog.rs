//! The terminal half: the chooser itself.
//!
//! Both questions a save asks — which directory, and what name — are on screen
//! at once and both stay live. That is the whole point of the project: a file
//! browser can only answer the first, which is why saving through one never
//! worked.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::{execute, terminal::SetTitle};
use ratatui::widgets::ListState;

use crate::field::Field;
use crate::model::{self, Entry, Fuzzy, Kind, SaveTarget};
use crate::protocol::{Mode, Request, Response, WirePath};
use crate::ui;

pub fn run(request_path: &Path, response_path: &Path) -> Result<()> {
    let raw = std::fs::read(request_path)
        .with_context(|| format!("reading {}", request_path.display()))?;
    let request: Request = serde_json::from_slice(&raw).context("parsing the request")?;

    let mut app = App::new(request)?;
    let response = ratatui::run(|terminal| app.event_loop(terminal))?;

    // The daemon reads an empty or absent file as a cancel, so a crash before
    // this point declines the dialog rather than hanging the application.
    let encoded = serde_json::to_vec(&response).context("encoding the response")?;
    std::fs::write(response_path, encoded)
        .with_context(|| format!("writing {}", response_path.display()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Name,
}

/// A line in the browser. `..` is not an entry on disk, and keeping it out of
/// the listing means filtering never has to remember to exempt it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Parent,
    Entry(usize),
}

pub struct App {
    pub request: Request,
    pub home: PathBuf,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub rows: Vec<Row>,
    pub list: ListState,
    pub query: String,
    pub name: Field,
    /// Some when the path bar is being edited, under `Ctrl-L`.
    pub path: Option<Field>,
    pub focus: Focus,
    pub show_hidden: bool,
    /// Set by the first `Enter` on a name that already exists; the second one
    /// goes through. Cleared by anything that changes what would be written.
    pub armed: bool,
    pub target: SaveTarget,
    pub message: Option<String>,
    fuzzy: Fuzzy,
    done: Option<Response>,
}

impl App {
    pub fn new(request: Request) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let dir = request.start_dir.to_path_buf();
        let name = Field::new(request.current_name.clone().unwrap_or_default());

        let mut app = Self {
            focus: if request.current_name.is_some() {
                Focus::Name
            } else {
                Focus::List
            },
            request,
            home,
            dir,
            entries: Vec::new(),
            rows: Vec::new(),
            list: ListState::default(),
            query: String::new(),
            name,
            path: None,
            show_hidden: false,
            armed: false,
            target: SaveTarget::Empty,
            message: None,
            fuzzy: Fuzzy::default(),
            done: None,
        };
        app.list.select(None);
        app.reload(None);
        Ok(app)
    }

    // ------------------------------------------------------------- queries

    pub fn saving(&self) -> bool {
        matches!(self.request.mode, Mode::SaveFile)
    }

    /// Whether the answer is a directory rather than a file. `SaveFiles` counts:
    /// the application already knows the names, so the only question left is
    /// where they go.
    pub fn picking_dir(&self) -> bool {
        matches!(
            self.request.mode,
            Mode::OpenDirectory { .. } | Mode::SaveFiles { .. }
        )
    }

    pub fn title(&self) -> String {
        if !self.request.title.is_empty() {
            return self.request.title.clone();
        }
        match &self.request.mode {
            Mode::OpenFile { multiple: false } => "Open file",
            Mode::OpenFile { .. } => "Open files",
            Mode::OpenDirectory { multiple: false } => "Choose folder",
            Mode::OpenDirectory { .. } => "Choose folders",
            Mode::SaveFile => "Save file",
            Mode::SaveFiles { .. } => "Save into",
        }
        .to_owned()
    }

    /// The application's word for the commit action, without GTK's mnemonic
    /// underscore.
    pub fn accept_label(&self) -> String {
        match &self.request.accept_label {
            Some(label) if !label.is_empty() => label.replace('_', ""),
            _ if self.saving() => "save".to_owned(),
            _ if self.picking_dir() => "choose".to_owned(),
            _ => "open".to_owned(),
        }
    }

    fn selected(&self) -> Option<Row> {
        self.list.selected().and_then(|i| self.rows.get(i).copied())
    }

    fn entry_path(&self, index: usize) -> PathBuf {
        self.dir.join(&self.entries[index].name)
    }

    // ------------------------------------------------------------- reading

    /// Re-lists the current directory. `keep` names the entry to put the cursor
    /// on, which is how stepping up to the parent lands on the directory just
    /// left rather than at the top.
    fn reload(&mut self, keep: Option<&OsStr>) {
        match model::list_dir(&self.dir) {
            Ok(entries) => {
                self.entries = entries;
                self.message = None;
            }
            Err(err) => {
                self.entries.clear();
                self.message = Some(err.to_string());
            }
        }
        self.refilter();
        if let Some(name) = keep {
            let found = self.rows.iter().position(|row| match row {
                Row::Entry(i) => self.entries[*i].name == name,
                Row::Parent => false,
            });
            if let Some(at) = found {
                self.list.select(Some(at));
            }
        }
        self.refresh_target();
    }

    fn refilter(&mut self) {
        let visible = self
            .fuzzy
            .visible(&self.entries, &self.query, self.show_hidden);
        self.rows.clear();
        if self.dir.parent().is_some() {
            self.rows.push(Row::Parent);
        }
        self.rows.extend(visible.into_iter().map(Row::Entry));

        let at = self.list.selected().unwrap_or_else(|| self.first_entry());
        self.list
            .select((!self.rows.is_empty()).then(|| at.min(self.rows.len() - 1)));
    }

    /// `..` is where you came from, not where you are going, so the cursor
    /// starts below it whenever there is anything below it.
    fn first_entry(&self) -> usize {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Entry(_)))
            .unwrap_or(0)
    }

    fn refresh_target(&mut self) {
        self.target = if self.saving() {
            model::save_target(&self.dir, &self.home, OsStr::new(self.name.text()))
        } else {
            SaveTarget::Empty
        };
    }

    // ------------------------------------------------------------ movement

    fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        let at = self.list.selected().unwrap_or(0) as isize;
        self.list.select(Some((at + delta).clamp(0, last) as usize));
    }

    fn go_to(&mut self, dir: PathBuf, keep: Option<OsString>) {
        self.dir = dir;
        self.query.clear();
        self.list.select(None);
        self.reload(keep.as_deref());
    }

    fn go_to_parent(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let leaving = self.dir.file_name().map(ToOwned::to_owned);
            self.go_to(parent, leaving);
        }
    }

    // ------------------------------------------------------------ answering

    fn cancel(&mut self) {
        self.done = Some(Response::Cancelled);
    }

    fn answer(&mut self, paths: Vec<PathBuf>) {
        self.done = Some(Response::Selected {
            paths: paths.iter().map(WirePath::from).collect(),
            filter: self.request.current_filter,
        });
    }

    /// `Enter` in the list, and `Right`.
    fn activate(&mut self) {
        match self.selected() {
            None => {}
            Some(Row::Parent) => self.go_to_parent(),
            Some(Row::Entry(i)) if self.entries[i].kind == Kind::Dir => {
                self.go_to(self.entry_path(i), None);
            }
            Some(Row::Entry(i)) if self.saving() => {
                // Copying the name is as far as this goes; committing is the
                // Name field's job. `Enter` meaning both "descend" and "commit"
                // in one place is what made the browser this replaces unusable
                // for saving.
                self.name
                    .set(self.entries[i].name.to_string_lossy().into_owned());
                self.focus = Focus::Name;
                self.armed = false;
                self.refresh_target();
            }
            Some(Row::Entry(i)) if !self.picking_dir() => {
                let path = self.entry_path(i);
                self.answer(vec![path]);
            }
            Some(Row::Entry(_)) => {}
        }
    }

    /// `Ctrl-S`, and `Enter` in the Name field.
    fn commit(&mut self) {
        if self.picking_dir() {
            return self.commit_directory();
        }
        if !self.saving() {
            // Open mode: the answer is whatever the cursor is on, and a
            // directory is not an answer.
            if let Some(Row::Entry(i)) = self.selected()
                && self.entries[i].kind == Kind::File
            {
                let path = self.entry_path(i);
                self.answer(vec![path]);
            }
            return;
        }

        match self.target.clone() {
            SaveTarget::Overwrite(path) => {
                if !self.armed {
                    self.armed = true;
                    return;
                }
                self.answer(vec![path]);
            }
            SaveTarget::New { path, parents } => {
                // The directories are made; the file is not. A save dialog
                // answers where to write and the application does the writing —
                // creating the file here is what littered this machine with
                // stub files under the setup being replaced.
                for parent in &parents {
                    if let Err(err) = std::fs::create_dir(parent) {
                        self.message = Some(format!("{}: {err}", parent.display()));
                        return;
                    }
                }
                self.answer(vec![path]);
            }
            // The hint under the field already says why these cannot be
            // committed, so refusing silently is not a silent refusal.
            SaveTarget::Empty | SaveTarget::IsDirectory(_) | SaveTarget::Blocked { .. } => {}
        }
    }

    fn commit_directory(&mut self) {
        let dir = match self.selected() {
            Some(Row::Entry(i)) if self.entries[i].kind == Kind::Dir => self.entry_path(i),
            _ => self.dir.clone(),
        };
        match &self.request.mode {
            Mode::SaveFiles { names } => {
                let paths = names
                    .iter()
                    .map(|name| dir.join(name.to_path_buf()))
                    .collect();
                self.answer(paths);
            }
            _ => self.answer(vec![dir]),
        }
    }

    // --------------------------------------------------------------- input

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<Response> {
        let _ = execute!(std::io::stdout(), SetTitle(self.title()));

        while self.done.is_none() {
            terminal.draw(|frame| ui::draw(frame, self))?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                _ => {}
            }
        }
        Ok(self.done.take().unwrap_or(Response::Cancelled))
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if ctrl && key.code == KeyCode::Char('c') {
            return self.cancel();
        }
        if self.path.is_some() {
            return self.on_path_key(key);
        }

        match key.code {
            KeyCode::Char('l') if ctrl => {
                self.path = Some(Field::new(abbreviate(&self.dir, &self.home)));
                return;
            }
            KeyCode::Char('h') if alt => {
                self.show_hidden = !self.show_hidden;
                self.refilter();
                return;
            }
            KeyCode::Char('s') if ctrl => return self.commit(),
            KeyCode::Enter if alt => return self.commit(),
            KeyCode::Tab | KeyCode::BackTab if self.saving() => {
                self.focus = match self.focus {
                    Focus::List => Focus::Name,
                    Focus::Name => Focus::List,
                };
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::List => self.on_list_key(key),
            Focus::Name => self.on_name_key(key),
        }
    }

    fn on_list_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            // Clearing the query first: a stray keystroke in the list should
            // cost one more keypress, not the whole dialog.
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.refilter();
            }
            KeyCode::Esc => self.cancel(),
            KeyCode::Enter | KeyCode::Right => self.activate(),
            KeyCode::Left => self.go_to_parent(),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Backspace if self.query.is_empty() => self.go_to_parent(),
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter();
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                self.refilter();
            }
            _ => {}
        }
    }

    fn on_name_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => return self.cancel(),
            KeyCode::Enter => return self.commit(),
            _ => {}
        }
        if self.name.handle(key) {
            // Anything that changes what would be written un-arms the
            // overwrite, so the second Enter can only ever confirm the name the
            // first one warned about.
            self.armed = false;
            self.refresh_target();
        }
    }

    fn on_path_key(&mut self, key: KeyEvent) {
        let Some(field) = &mut self.path else { return };
        match key.code {
            KeyCode::Esc => {
                self.path = None;
            }
            KeyCode::Enter => {
                let typed = field.text().to_owned();
                self.path = None;
                self.go_to_typed(&typed);
            }
            _ => {
                field.handle(key);
            }
        }
    }

    /// A path typed into the bar. If it names a file, the directory changes and
    /// the filename lands in the Name field — typing a full path is a way of
    /// answering both questions at once.
    fn go_to_typed(&mut self, typed: &str) {
        let Some(path) = model::resolve(&self.dir, &self.home, OsStr::new(typed)) else {
            return;
        };
        if path.is_dir() {
            return self.go_to(path, None);
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if !parent.is_dir() {
            self.message = Some(format!("{} does not exist", abbreviate(parent, &self.home)));
            return;
        }
        self.go_to(parent.to_path_buf(), Some(name.to_owned()));
        if self.saving() {
            self.name.set(name.to_string_lossy().into_owned());
            self.focus = Focus::Name;
            self.armed = false;
            self.refresh_target();
        }
    }
}

/// Writes `$HOME` back as `~`, so the path bar reads as a path rather than as a
/// column of `/home/<user>`.
pub fn abbreviate(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
