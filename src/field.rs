//! A one-line editable text field.
//!
//! The Name field and the path bar are the same widget pointed at different
//! things. Kept apart from the drawing so the editing rules — which are the
//! part that can be wrong in ways nobody notices until a filename is mangled —
//! can be tested without a terminal.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct Field {
    text: String,
    /// Byte index, always on a character boundary.
    cursor: usize,
}

impl Field {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the contents, leaving the cursor at the end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    /// Returns whether the key was consumed. `Enter` never is: committing is
    /// the caller's decision and depends on which field this is.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if let Some(prev) = self.prev_boundary(self.cursor) {
                    self.text.replace_range(prev..self.cursor, "");
                    self.cursor = prev;
                }
            }
            KeyCode::Delete => {
                if let Some(next) = self.next_boundary(self.cursor) {
                    self.text.replace_range(self.cursor..next, "");
                }
            }
            KeyCode::Left if !ctrl && !alt => {
                self.cursor = self.prev_boundary(self.cursor).unwrap_or(0);
            }
            KeyCode::Right if !ctrl && !alt => {
                self.cursor = self.next_boundary(self.cursor).unwrap_or(self.text.len());
            }
            KeyCode::Left | KeyCode::Char('b') if alt => self.cursor = self.word_start(),
            KeyCode::Right | KeyCode::Char('f') if alt => self.cursor = self.word_end(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char('u') if ctrl => {
                self.text.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.text.truncate(self.cursor),
            KeyCode::Char('w') if ctrl => {
                let start = self.word_start();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            _ => return false,
        }
        true
    }

    /// The slice to draw in a field `width` columns wide, and the column the
    /// cursor sits at within it.
    ///
    /// Scrolls only as far as it must: a name that fits is never shifted, so
    /// the beginning of the path stays put while typing at the end.
    pub fn view(&self, width: usize) -> (&str, usize) {
        let cursor_col = columns(&self.text[..self.cursor]);
        if columns(&self.text) < width {
            return (&self.text, cursor_col);
        }

        // Keep a column free after the cursor for the cursor block itself.
        let first = cursor_col.saturating_sub(width.saturating_sub(1));
        let start = byte_at_column(&self.text, first);
        (
            take_columns(&self.text[start..], width),
            cursor_col - columns(&self.text[..start]),
        )
    }

    fn prev_boundary(&self, from: usize) -> Option<usize> {
        self.text[..from]
            .chars()
            .next_back()
            .map(|c| from - c.len_utf8())
    }

    fn next_boundary(&self, from: usize) -> Option<usize> {
        self.text[from..]
            .chars()
            .next()
            .map(|c| from + c.len_utf8())
    }

    /// Skips separators, then the run of non-separators before the cursor.
    fn word_start(&self) -> usize {
        let mut at = self.cursor;
        while let Some(prev) = self.prev_boundary(at) {
            if !is_separator(self.text[prev..].chars().next().unwrap()) {
                break;
            }
            at = prev;
        }
        while let Some(prev) = self.prev_boundary(at) {
            if is_separator(self.text[prev..].chars().next().unwrap()) {
                break;
            }
            at = prev;
        }
        at
    }

    fn word_end(&self) -> usize {
        let mut at = self.cursor;
        while let Some(next) = self.next_boundary(at) {
            if !is_separator(self.text[at..].chars().next().unwrap()) {
                break;
            }
            at = next;
        }
        while let Some(next) = self.next_boundary(at) {
            if is_separator(self.text[at..].chars().next().unwrap()) {
                break;
            }
            at = next;
        }
        at
    }
}

/// `/` counts, so word motions step through a path a component at a time.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '/' | '.' | '-' | '_')
}

fn columns(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

fn byte_at_column(s: &str, column: usize) -> usize {
    let mut at = 0;
    for (i, c) in s.char_indices() {
        if at >= column {
            return i;
        }
        at += c.width().unwrap_or(0);
    }
    s.len()
}

fn take_columns(s: &str, width: usize) -> &str {
    let mut at = 0;
    for (i, c) in s.char_indices() {
        let w = c.width().unwrap_or(0);
        if at + w > width {
            return &s[..i];
        }
        at += w;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(s: &str) -> Field {
        let mut field = Field::default();
        for c in s.chars() {
            field.handle(key(KeyCode::Char(c)));
        }
        field
    }

    #[test]
    fn typing_appends() {
        assert_eq!(typed("board.corkboard").text(), "board.corkboard");
    }

    #[test]
    fn a_new_field_puts_the_cursor_at_the_end() {
        let mut field = Field::new("a.txt");
        field.handle(key(KeyCode::Char('!')));
        assert_eq!(field.text(), "a.txt!");
    }

    #[test]
    fn editing_happens_where_the_cursor_is() {
        let mut field = Field::new("a.txt");
        field.handle(key(KeyCode::Home));
        field.handle(key(KeyCode::Char('x')));
        assert_eq!(field.text(), "xa.txt");
    }

    #[test]
    fn backspace_removes_whole_characters_not_bytes() {
        let mut field = typed("café");
        field.handle(key(KeyCode::Backspace));
        assert_eq!(field.text(), "caf");
    }

    #[test]
    fn arrows_step_over_whole_characters() {
        let mut field = typed("é!");
        field.handle(key(KeyCode::Left));
        field.handle(key(KeyCode::Left));
        field.handle(key(KeyCode::Char('x')));
        assert_eq!(field.text(), "xé!");
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        let mut field = typed("ab");
        for _ in 0..5 {
            field.handle(key(KeyCode::Left));
        }
        field.handle(key(KeyCode::Char('x')));
        assert_eq!(field.text(), "xab");

        for _ in 0..5 {
            field.handle(key(KeyCode::Right));
        }
        field.handle(key(KeyCode::Char('y')));
        assert_eq!(field.text(), "xaby");
    }

    #[test]
    fn kill_line_cuts_at_the_cursor() {
        let mut field = Field::new("a/b/c.txt");
        field.handle(key(KeyCode::Home));
        field.handle(ctrl('k'));
        assert_eq!(field.text(), "");

        let mut field = Field::new("a/b/c.txt");
        field.handle(ctrl('u'));
        assert_eq!(field.text(), "");
    }

    #[test]
    fn word_delete_steps_through_a_path_a_component_at_a_time() {
        let mut field = Field::new("photos/holiday/beach.png");
        field.handle(ctrl('w'));
        assert_eq!(field.text(), "photos/holiday/beach.");
        field.handle(ctrl('w'));
        assert_eq!(field.text(), "photos/holiday/");
        field.handle(ctrl('w'));
        assert_eq!(field.text(), "photos/");
    }

    #[test]
    fn enter_is_left_for_the_caller() {
        let mut field = Field::new("a");
        assert!(!field.handle(key(KeyCode::Enter)));
        assert_eq!(field.text(), "a");
    }

    #[test]
    fn a_field_that_fits_is_never_scrolled() {
        let field = Field::new("a.txt");
        assert_eq!(field.view(40), ("a.txt", 5));
    }

    #[test]
    fn a_long_field_scrolls_to_keep_the_cursor_in_view() {
        let field = Field::new("abcdefghij");
        let (visible, cursor) = field.view(5);
        assert_eq!(visible, "ghij");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn scrolling_back_to_the_start_shows_the_start() {
        let mut field = Field::new("abcdefghij");
        field.handle(key(KeyCode::Home));
        let (visible, cursor) = field.view(5);
        assert_eq!(visible, "abcde");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn wide_characters_count_as_two_columns() {
        let field = Field::new("日本語");
        assert_eq!(field.view(10), ("日本語", 6));
        // Only three of the four columns are usable: the cursor sits past the
        // end of the text and needs one of its own.
        let (visible, cursor) = field.view(4);
        assert_eq!(visible, "語");
        assert_eq!(cursor, 2);
    }
}
