//! The terminal half: the chooser itself.
//!
//! Both questions a save asks — which directory, and what name — are on screen
//! at once and both stay live. That is the whole point of the project: a file
//! browser can only answer the first, which is why saving through one never
//! worked.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::{execute, terminal::SetTitle};
use ratatui::widgets::ListState;

use crate::field::Field;
use crate::model::{self, Entry, Fuzzy, Kind, SaveTarget};
use crate::protocol::{Filter, Mode, Request, Response, WirePath};
use crate::ui;

/// How many directories the jump list will hold. A home directory with a
/// monorepo checked out has more, and the ones past this point are build trees
/// nobody jumps to by name.
const JUMP_BUDGET: usize = 40_000;
/// How many of them are ranked onto the screen at once.
const JUMP_SHOWN: usize = 200;

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

/// What the row under the listing is for, when there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bottom {
    /// The Name field: the second question a save asks.
    Name,
    /// The names the application is about to write, under `SaveFiles`.
    Writing,
    /// What has been marked, in the modes that take more than one answer.
    Marked,
}

/// The `Ctrl-P` jump list, borrowing the path bar for its query.
pub struct Jump {
    pub field: Field,
    /// Indices into `App::dirs`, best first.
    pub shown: Vec<usize>,
    pub list: ListState,
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
    /// Which of the application's filters is in force. `None` is filepick's own
    /// "all files": the portal has no such entry, but being unable to see a
    /// file you know is there is worse than a filter that lapses.
    pub filter: Option<usize>,
    /// Paths marked with `Space`. Marks survive changing directory — answering
    /// with files from one directory only is a limitation of the dialogs this
    /// replaces, not something worth copying.
    pub marks: BTreeSet<PathBuf>,
    /// Every directory under `$HOME`, once the walk has finished.
    pub dirs: Vec<PathBuf>,
    /// Some while the jump list is open, under `Ctrl-P`.
    pub jump: Option<Jump>,
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
    scan: Option<mpsc::Receiver<Vec<PathBuf>>>,
    done: Option<Response>,
}

impl App {
    pub fn new(request: Request) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let dir = request.start_dir.to_path_buf();
        let name = Field::new(request.current_name.clone().unwrap_or_default());
        let request_filter = request.current_filter;

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
            filter: request_filter,
            marks: BTreeSet::new(),
            dirs: Vec::new(),
            jump: None,
            path: None,
            show_hidden: false,
            armed: false,
            target: SaveTarget::Empty,
            message: None,
            fuzzy: Fuzzy::default(),
            scan: None,
            done: None,
        };
        app.list.select(None);
        app.reload(None);
        app.suggest_extension();
        Ok(app)
    }

    /// `Untitled` under a `*.png` filter becomes `Untitled.png`. Only ever
    /// added, never swapped, so a name that already says what it is survives —
    /// and so does the extension the application itself chose.
    fn suggest_extension(&mut self) {
        if !self.saving() || self.name.text().is_empty() || self.name.text().contains('.') {
            return;
        }
        let extension: Option<String> = self.active_filter().and_then(model::filters::extension);
        if let Some(extension) = extension {
            self.name.set(format!("{}.{extension}", self.name.text()));
            self.refresh_target();
        }
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

    /// Whether the request takes more than one answer.
    pub fn multiple(&self) -> bool {
        matches!(
            self.request.mode,
            Mode::OpenFile { multiple: true } | Mode::OpenDirectory { multiple: true }
        )
    }

    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// What the row under the listing says, if anything.
    pub fn bottom(&self) -> Option<Bottom> {
        if self.saving() {
            Some(Bottom::Name)
        } else if matches!(self.request.mode, Mode::SaveFiles { .. }) {
            Some(Bottom::Writing)
        } else if !self.marks.is_empty() {
            Some(Bottom::Marked)
        } else {
            None
        }
    }

    pub fn active_filter(&self) -> Option<&Filter> {
        self.filter.and_then(|i| self.request.filters.get(i))
    }

    /// The name of the filter in force, or `None` when the application sent no
    /// filters and there is nothing to cycle.
    pub fn filter_label(&self) -> Option<String> {
        if self.request.filters.is_empty() {
            return None;
        }
        Some(match self.active_filter() {
            Some(filter) => filter.label.clone(),
            None => "all files".to_owned(),
        })
    }

    /// The directory a commit would answer with: the one under the cursor if
    /// the cursor is on one, otherwise the one being browsed.
    pub fn chosen_dir(&self) -> PathBuf {
        match self.selected() {
            Some(Row::Entry(i)) if self.entries[i].kind == Kind::Dir => self.entry_path(i),
            _ => self.dir.clone(),
        }
    }

    /// The names `SaveFiles` is about to write, and how many of them already
    /// exist where they would go. Knowing that before choosing the directory is
    /// the whole reason the preview is on screen.
    pub fn writing(&self) -> (Vec<String>, usize) {
        let Mode::SaveFiles { names } = &self.request.mode else {
            return (Vec::new(), 0);
        };
        let dir = self.chosen_dir();
        let mut existing = 0;
        let names = names
            .iter()
            .map(|name| {
                let name = name.to_path_buf();
                if dir.join(&name).exists() {
                    existing += 1;
                }
                name.display().to_string()
            })
            .collect();
        (names, existing)
    }

    pub fn marked(&self) -> Vec<String> {
        self.marks
            .iter()
            .map(|path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
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
        // Directories are never filtered out. A filter says which files the
        // application can read, and hiding the way to them would make a filter
        // that matches nothing here into a dialog with no way forward.
        let filter = self.filter.and_then(|i| self.request.filters.get(i));
        let visible: Vec<usize> = visible
            .into_iter()
            .filter(|i| {
                let entry = &self.entries[*i];
                entry.kind == Kind::Dir
                    || filter.is_none_or(|filter| model::filters::matches(filter, &entry.name))
            })
            .collect();
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

    fn cycle_filter(&mut self) {
        if self.request.filters.is_empty() {
            return;
        }
        self.filter = match self.filter {
            None => Some(0),
            Some(i) if i + 1 < self.request.filters.len() => Some(i + 1),
            Some(_) => None,
        };
        self.refilter();
    }

    /// Recomputes what committing would write, which is also what un-arms an
    /// overwrite: the arming belongs to one target, so a new one cannot inherit
    /// it. Changing directory recomputes the target too, which is the case that
    /// scattering this over the call sites got wrong.
    fn refresh_target(&mut self) {
        self.armed = false;
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

    /// `Space`. Only the kind of thing the request is asking for can be marked:
    /// marking a file in a directory chooser would promise an answer the portal
    /// has no way to give.
    fn toggle_mark(&mut self) {
        let Some(Row::Entry(i)) = self.selected() else {
            return;
        };
        let is_dir = self.entries[i].kind == Kind::Dir;
        if is_dir != self.picking_dir() {
            return;
        }
        let path = self.entry_path(i);
        if !self.marks.remove(&path) {
            self.marks.insert(path);
        }
        self.move_by(1);
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

    // ------------------------------------------------------------- jumping

    fn open_jump(&mut self) {
        if self.dirs.is_empty() && self.scan.is_none() {
            let (tx, rx) = mpsc::channel();
            let home = self.home.clone();
            // The walk is the one thing here slow enough to be felt on a large
            // home directory, so it runs beside the dialog rather than in front
            // of it, and the list fills in when it arrives.
            std::thread::spawn(move || {
                let _ = tx.send(model::walk_dirs(&home, JUMP_BUDGET));
            });
            self.scan = Some(rx);
        }
        self.jump = Some(Jump {
            field: Field::default(),
            shown: Vec::new(),
            list: ListState::default(),
        });
        self.rejump();
    }

    fn rejump(&mut self) {
        let Self {
            jump, fuzzy, dirs, ..
        } = self;
        let Some(jump) = jump else { return };
        jump.shown = fuzzy.rank(dirs, jump.field.text(), JUMP_SHOWN);
        jump.list.select((!jump.shown.is_empty()).then_some(0));
    }

    fn collect_scan(&mut self) {
        let Some(rx) = &self.scan else { return };
        match rx.try_recv() {
            Ok(dirs) => {
                self.dirs = dirs;
                self.scan = None;
                self.rejump();
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.scan = None,
        }
    }

    fn jump_move(&mut self, delta: isize) {
        let Some(jump) = &mut self.jump else { return };
        if jump.shown.is_empty() {
            return;
        }
        let last = jump.shown.len() as isize - 1;
        let at = jump.list.selected().unwrap_or(0) as isize;
        jump.list.select(Some((at + delta).clamp(0, last) as usize));
    }

    // ------------------------------------------------------------ answering

    fn cancel(&mut self) {
        self.done = Some(Response::Cancelled);
    }

    fn answer(&mut self, paths: Vec<PathBuf>) {
        self.done = Some(Response::Selected {
            paths: paths.iter().map(WirePath::from).collect(),
            filter: self.filter,
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
                self.refresh_target();
            }
            Some(Row::Entry(i)) if !self.picking_dir() => {
                let paths = self.answer_or(self.entry_path(i));
                self.answer(paths);
            }
            Some(Row::Entry(_)) => {}
        }
    }

    /// The marks, or `fallback` if nothing is marked.
    ///
    /// Marks outrank the cursor even when a key is aimed at a row, because the
    /// cursor is where navigating left it rather than anything chosen. Adding
    /// the row under it costs one more `Space` — the same key that put the rest
    /// of them there.
    fn answer_or(&self, fallback: PathBuf) -> Vec<PathBuf> {
        if self.marks.is_empty() {
            vec![fallback]
        } else {
            self.marks.iter().cloned().collect()
        }
    }

    /// `Ctrl-S`, and `Enter` in the Name field.
    fn commit(&mut self) {
        if self.multiple() && !self.marks.is_empty() {
            let paths = self.marks.iter().cloned().collect();
            return self.answer(paths);
        }
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
        let dir = self.chosen_dir();
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
            self.collect_scan();
            terminal.draw(|frame| ui::draw(frame, self))?;
            // A walk in flight is the only reason to wake without a keypress.
            if self.scan.is_some() && !event::poll(Duration::from_millis(60))? {
                continue;
            }
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
        if self.jump.is_some() {
            return self.on_jump_key(key);
        }
        if self.path.is_some() {
            return self.on_path_key(key);
        }

        match key.code {
            KeyCode::Char('p') if ctrl => return self.open_jump(),
            KeyCode::Char('f') if ctrl => return self.cycle_filter(),
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
            KeyCode::Char(' ') if self.multiple() => self.toggle_mark(),
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
            self.refresh_target();
        }
    }

    fn on_jump_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => return self.jump = None,
            KeyCode::Char('p') if ctrl => return self.jump = None,
            KeyCode::Up => return self.jump_move(-1),
            KeyCode::Down => return self.jump_move(1),
            KeyCode::PageUp => return self.jump_move(-10),
            KeyCode::PageDown => return self.jump_move(10),
            KeyCode::Enter => {
                let chosen = self.jump.as_ref().and_then(|jump| {
                    let at = jump.list.selected()?;
                    self.dirs.get(*jump.shown.get(at)?).cloned()
                });
                self.jump = None;
                if let Some(dir) = chosen {
                    self.go_to(dir, None);
                }
                return;
            }
            _ => {}
        }

        if let Some(jump) = &mut self.jump
            && jump.field.handle(key)
        {
            self.rejump();
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
