//! What a key does, and which key does it.
//!
//! Every binding is data in one table. The dispatcher looks a key up in it and
//! the footer looks a key up *by* the action it advertises, so the two cannot
//! disagree about which key does what — they did, before this existed, and the
//! footer was the one telling the lie.
//!
//! Keys are written the way the footer draws them, `^S` and `M-h` and `Enter`,
//! so that a line copied off the screen into the config file is already valid.

use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Which set of bindings is in force. The overlays are their own layer rather
/// than an addition to one: while the jump list is open, `Ctrl-P` closes it
/// instead of opening it again, and nothing else global applies at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Consulted for [`Context::List`] and [`Context::Name`], never for an overlay.
    Global,
    List,
    Name,
    Jump,
    Path,
}

impl Context {
    const ALL: [Self; 5] = [Self::Global, Self::List, Self::Name, Self::Jump, Self::Path];

    /// The name of its table in the config file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::List => "list",
            Self::Name => "name",
            Self::Jump => "jump",
            Self::Path => "path",
        }
    }

    /// The layers a key is looked up in, nearest first.
    fn layers(self) -> &'static [Self] {
        match self {
            Self::List => &[Self::List, Self::Global],
            Self::Name => &[Self::Name, Self::Global],
            other => match other {
                Self::Global => &[Self::Global],
                Self::Jump => &[Self::Jump],
                _ => &[Self::Path],
            },
        }
    }

    /// Whether binding `action` here means anything. Rejecting the rest is what
    /// turns a binding that would quietly do nothing into a message.
    fn allows(self, action: Action) -> bool {
        use Action::*;
        match self {
            Self::Global => matches!(
                action,
                Commit | Cancel | Jump | Filter | Path | Hidden | Pane
            ),
            Self::List => matches!(
                action,
                Activate
                    | Parent
                    | Up
                    | Down
                    | PageUp
                    | PageDown
                    | First
                    | Last
                    | Mark
                    | Dismiss
                    | Erase
                    | Clear
                    | Commit
                    | Cancel
                    | Jump
                    | Filter
                    | Path
                    | Hidden
                    | Pane
            ),
            Self::Name => matches!(
                action,
                Commit | Cancel | Jump | Filter | Path | Hidden | Pane
            ),
            Self::Jump => matches!(action, Go | Close | Up | Down | PageUp | PageDown),
            Self::Path => matches!(action, Go | Close),
        }
    }
}

/// Something a key can be bound to.
///
/// Deliberately not one variant per key press: `Dismiss` is `Esc`'s whole
/// meaning in the list — clear the filter, and only cancel once there is none —
/// because a user rebinding it wants that behaviour, not the first half of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Commit,
    Cancel,
    Jump,
    Filter,
    Path,
    Hidden,
    Pane,
    Activate,
    Parent,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    Mark,
    Dismiss,
    Erase,
    Clear,
    /// Accepts an overlay: goes to the highlighted directory, or the typed path.
    Go,
    /// Abandons an overlay, leaving the browser where it was.
    Close,
}

/// The name the config file calls each action. One name per action, so that a
/// misspelling can be reported against a list rather than guessed at.
const NAMES: &[(&str, Action)] = &[
    ("commit", Action::Commit),
    ("cancel", Action::Cancel),
    ("jump", Action::Jump),
    ("filter", Action::Filter),
    ("path", Action::Path),
    ("hidden", Action::Hidden),
    ("pane", Action::Pane),
    ("activate", Action::Activate),
    ("parent", Action::Parent),
    ("up", Action::Up),
    ("down", Action::Down),
    ("page-up", Action::PageUp),
    ("page-down", Action::PageDown),
    ("first", Action::First),
    ("last", Action::Last),
    ("mark", Action::Mark),
    ("dismiss", Action::Dismiss),
    ("erase", Action::Erase),
    ("clear", Action::Clear),
    ("go", Action::Go),
    ("close", Action::Close),
];

impl Action {
    fn parse(name: &str) -> Option<Self> {
        NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, action)| *action)
    }
}

/// A key press, with the modifiers that distinguish one binding from another.
///
/// `Shift` is not among them: it is already in the character, and a listing
/// filtered by typing needs `A` and `a` to reach the same binding table by
/// missing it entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    code: KeyCode,
    ctrl: bool,
    alt: bool,
}

impl Key {
    pub fn from_event(event: KeyEvent) -> Self {
        let code = match event.code {
            // `Ctrl-S` arrives lowercase and `^S` is written uppercase; folding
            // here means the table holds one entry rather than two.
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            code => code,
        };
        Self {
            code,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// The character this key would type, if it would type one.
    pub fn typed(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if !self.ctrl && !self.alt => Some(c),
            _ => None,
        }
    }

    /// The one key that is never in the table: a way out that a config file
    /// can take away is not a way out.
    pub fn interrupt() -> Self {
        Self {
            code: KeyCode::Char('c'),
            ctrl: true,
            alt: false,
        }
    }

    pub fn parse(spec: &str) -> Option<Self> {
        let mut rest = spec.trim();
        let (mut ctrl, mut alt) = (false, false);
        loop {
            let stripped = ["^", "C-", "ctrl-", "Ctrl-"]
                .iter()
                .find_map(|p| rest.strip_prefix(p))
                .map(|r| (r, true, false))
                .or_else(|| {
                    ["M-", "A-", "alt-", "Alt-"]
                        .iter()
                        .find_map(|p| rest.strip_prefix(p))
                        .map(|r| (r, false, true))
                });
            match stripped {
                // A bare `^` or `M-` names no key at all.
                Some((r, _, _)) if r.is_empty() => return None,
                Some((r, c, a)) => {
                    rest = r;
                    ctrl |= c;
                    alt |= a;
                }
                None => break,
            }
        }
        let code = code_from_name(rest)?;
        Some(Self { code, ctrl, alt })
    }
}

const NAMED: &[(&str, KeyCode)] = &[
    ("Enter", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("S-Tab", KeyCode::BackTab),
    ("Space", KeyCode::Char(' ')),
    ("Backspace", KeyCode::Backspace),
    ("Delete", KeyCode::Delete),
    ("Insert", KeyCode::Insert),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
];

fn code_from_name(name: &str) -> Option<KeyCode> {
    if let Some((_, code)) = NAMED
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
    {
        return Some(*code);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(KeyCode::Char(c.to_ascii_lowercase())),
        _ => None,
    }
}

impl fmt::Display for Key {
    /// The form the footer shows, which is also a form [`Key::parse`] accepts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("^")?;
        }
        if self.alt {
            f.write_str("M-")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            // `^s` is a hard shape to read in a row of words; `^S` is not.
            KeyCode::Char(c) if self.ctrl => write!(f, "{}", c.to_ascii_uppercase()),
            KeyCode::Char(c) => write!(f, "{c}"),
            code => match NAMED.iter().find(|(_, known)| *known == code) {
                Some((name, _)) => f.write_str(name),
                None => write!(f, "{code:?}"),
            },
        }
    }
}

/// Every binding filepick ships with. Written as the config file would write
/// them, so the defaults and a user's overrides go through the same parser.
const DEFAULTS: &[(Context, &str, Action)] = &[
    (Context::Global, "^S", Action::Commit),
    (Context::Global, "M-Enter", Action::Commit),
    (Context::Global, "^P", Action::Jump),
    (Context::Global, "^F", Action::Filter),
    (Context::Global, "^L", Action::Path),
    (Context::Global, "M-h", Action::Hidden),
    (Context::Global, "Tab", Action::Pane),
    (Context::Global, "S-Tab", Action::Pane),
    (Context::List, "Enter", Action::Activate),
    (Context::List, "Right", Action::Activate),
    (Context::List, "Left", Action::Parent),
    (Context::List, "Up", Action::Up),
    (Context::List, "Down", Action::Down),
    (Context::List, "PageUp", Action::PageUp),
    (Context::List, "PageDown", Action::PageDown),
    (Context::List, "Home", Action::First),
    (Context::List, "End", Action::Last),
    (Context::List, "Space", Action::Mark),
    (Context::List, "Esc", Action::Dismiss),
    (Context::List, "Backspace", Action::Erase),
    (Context::List, "^U", Action::Clear),
    (Context::Name, "Enter", Action::Commit),
    (Context::Name, "Esc", Action::Cancel),
    (Context::Jump, "Enter", Action::Go),
    (Context::Jump, "Esc", Action::Close),
    (Context::Jump, "^P", Action::Close),
    (Context::Jump, "Up", Action::Up),
    (Context::Jump, "Down", Action::Down),
    (Context::Jump, "PageUp", Action::PageUp),
    (Context::Jump, "PageDown", Action::PageDown),
    (Context::Path, "Enter", Action::Go),
    (Context::Path, "Esc", Action::Close),
];

#[derive(Debug, Clone)]
pub struct Keymap {
    /// One layer per [`Context`], in `Context::ALL` order. Ordered rather than
    /// keyed: the first binding for an action is the one the footer shows, so
    /// which one comes first is part of the answer.
    layers: [Vec<(Key, Action)>; 5],
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Self {
            layers: Default::default(),
        };
        for (context, spec, action) in DEFAULTS {
            let key = Key::parse(spec).expect("a default binding names a real key");
            map.layer_mut(*context).push((key, *action));
        }
        map
    }
}

impl Keymap {
    fn layer_mut(&mut self, context: Context) -> &mut Vec<(Key, Action)> {
        &mut self.layers[Context::ALL.iter().position(|c| *c == context).unwrap()]
    }

    fn layer(&self, context: Context) -> &[(Key, Action)] {
        &self.layers[Context::ALL.iter().position(|c| *c == context).unwrap()]
    }

    /// What this key is bound to here, if anything.
    pub fn action(&self, context: Context, key: Key) -> Option<Action> {
        context.layers().iter().find_map(|layer| {
            self.layer(*layer)
                .iter()
                .find(|(bound, _)| *bound == key)
                .map(|(_, action)| *action)
        })
    }

    /// The key to advertise for an action: the first one bound to it.
    pub fn key(&self, context: Context, action: Action) -> Option<Key> {
        context.layers().iter().find_map(|layer| {
            self.layer(*layer)
                .iter()
                .find(|(_, bound)| *bound == action)
                .map(|(key, _)| *key)
        })
    }

    /// Applies one `[keys.<context>]` table, reporting what it could not use.
    ///
    /// A binding replaces every default for its key in that layer and goes to
    /// the front, so that the footer advertises what the user chose rather than
    /// what was shipped.
    pub fn bind(&mut self, context: Context, spec: &str, name: &str) -> Result<(), String> {
        let Some(key) = Key::parse(spec) else {
            return Err(format!("keys.{}: {spec:?} is not a key", context.as_str()));
        };
        let Some(action) = Action::parse(name) else {
            return Err(format!(
                "keys.{}: there is no action called {name:?}",
                context.as_str()
            ));
        };
        if !context.allows(action) {
            return Err(format!(
                "keys.{}: {name:?} does nothing there",
                context.as_str()
            ));
        }
        let layer = self.layer_mut(context);
        layer.retain(|(bound, _)| *bound != key);
        layer.insert(0, (key, action));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &str) -> Key {
        Key::parse(spec).unwrap()
    }

    #[test]
    fn every_shipped_binding_parses() {
        // `Keymap::default` unwraps them, so this is what keeps that honest.
        for (_, spec, _) in DEFAULTS {
            assert!(Key::parse(spec).is_some(), "{spec}");
        }
    }

    #[test]
    fn a_key_reads_back_as_it_is_written() {
        for spec in ["^S", "M-h", "Enter", "Esc", "Space", "PageDown", "S-Tab"] {
            assert_eq!(key(spec).to_string(), spec);
        }
    }

    #[test]
    fn the_long_spellings_mean_the_same_thing() {
        assert_eq!(key("ctrl-s"), key("^S"));
        assert_eq!(key("alt-h"), key("M-h"));
        assert_eq!(key("C-M-x"), key("M-^x"));
    }

    #[test]
    fn nonsense_is_not_a_key() {
        for spec in ["", "^", "M-", "Retrun", "  "] {
            assert!(Key::parse(spec).is_none(), "{spec}");
        }
    }

    #[test]
    fn shift_is_not_part_of_a_binding() {
        let shifted = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(shifted), key("a"));
    }

    #[test]
    fn a_key_types_only_when_nothing_modifies_it() {
        assert_eq!(key("a").typed(), Some('a'));
        assert_eq!(key("Space").typed(), Some(' '));
        assert_eq!(key("^A").typed(), None);
        assert_eq!(key("Enter").typed(), None);
    }

    #[test]
    fn the_global_layer_is_reachable_from_the_panes_only() {
        let map = Keymap::default();
        assert_eq!(map.action(Context::List, key("^S")), Some(Action::Commit));
        assert_eq!(map.action(Context::Name, key("^S")), Some(Action::Commit));
        // An overlay is a different question; committing under it would answer
        // one the user is no longer being asked.
        assert_eq!(map.action(Context::Jump, key("^S")), None);
    }

    #[test]
    fn enter_means_a_different_thing_in_each_pane() {
        let map = Keymap::default();
        assert_eq!(
            map.action(Context::List, key("Enter")),
            Some(Action::Activate)
        );
        assert_eq!(
            map.action(Context::Name, key("Enter")),
            Some(Action::Commit)
        );
        assert_eq!(map.action(Context::Jump, key("Enter")), Some(Action::Go));
    }

    #[test]
    fn the_footer_can_ask_which_key_does_a_thing() {
        let map = Keymap::default();
        let commit = map.key(Context::List, Action::Commit).unwrap();
        assert_eq!(commit.to_string(), "^S");
    }

    #[test]
    fn a_binding_replaces_the_default_and_is_what_gets_advertised() {
        let mut map = Keymap::default();
        map.bind(Context::Global, "^W", "commit").unwrap();
        assert_eq!(map.action(Context::List, key("^W")), Some(Action::Commit));
        assert_eq!(
            map.key(Context::List, Action::Commit).unwrap().to_string(),
            "^W"
        );
        // The shipped key still works: binding a key says what that key does,
        // not what every other key stops doing.
        assert_eq!(map.action(Context::List, key("^S")), Some(Action::Commit));
    }

    #[test]
    fn rebinding_a_key_displaces_what_it_did() {
        let mut map = Keymap::default();
        map.bind(Context::List, "Enter", "mark").unwrap();
        assert_eq!(map.action(Context::List, key("Enter")), Some(Action::Mark));
    }

    #[test]
    fn a_binding_that_could_not_work_is_refused_rather_than_kept() {
        let mut map = Keymap::default();
        assert!(map.bind(Context::Name, "^X", "activate").is_err());
        assert!(map.bind(Context::List, "^X", "activte").is_err());
        assert!(map.bind(Context::List, "nonsense", "activate").is_err());
        assert_eq!(map.action(Context::List, key("^X")), None);
    }
}
