//! Everything the chooser knows that is not drawing.
//!
//! Listing a directory, working out what a typed name actually points at, and
//! deciding whether a file matches a filter are all decidable without a
//! terminal, so they live here and are tested directly. The only I/O is
//! `read_dir` and `metadata`.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------- listing

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    // Ordering is the display order: directories first.
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    pub size: u64,
    pub hidden: bool,
}

/// Reads `dir`, sorted, without the `..` the interface shows above it.
///
/// `..` is drawn by the view rather than returned here, so that filters and
/// fuzzy matching never have to remember to exempt it.
///
/// An entry that cannot be stat'd is still listed. Losing a file from the view
/// because its metadata was unreadable would be a worse failure than showing it
/// with a size of zero.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();

        // `DirEntry::metadata` does *not* follow symlinks on Unix, so a link to
        // a directory would list as a file and refuse to be entered. Stat the
        // path instead, falling back to the link itself when the target is gone.
        let meta = std::fs::metadata(entry.path()).or_else(|_| entry.metadata());

        let (kind, size) = match meta {
            Ok(meta) if meta.is_dir() => (Kind::Dir, 0),
            Ok(meta) => (Kind::File, meta.len()),
            Err(_) => (Kind::File, 0),
        };

        entries.push(Entry {
            hidden: name.as_bytes().starts_with(b"."),
            name,
            kind,
            size,
        });
    }
    sort(&mut entries);
    Ok(entries)
}

pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| name_order(&a.name, &b.name))
    });
}

/// Case-insensitive, with the raw bytes as a tiebreak so the order is total.
fn name_order(a: &OsStr, b: &OsStr) -> Ordering {
    let (la, lb) = (a.to_string_lossy(), b.to_string_lossy());
    la.chars()
        .flat_map(char::to_lowercase)
        .cmp(lb.chars().flat_map(char::to_lowercase))
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// SI units, matching what a GUI chooser shows.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ------------------------------------------------------------- resolution

/// Turns whatever is in the name field or the path bar into an absolute path.
///
/// `..` is folded lexically rather than by `canonicalize`, for two reasons: the
/// path may not exist yet, which is the normal case for a save; and `a/../b`
/// typed by a person means `b`, even when `a` is a symlink pointing elsewhere.
///
/// A leading `~` expands only as the whole input or before a `/`. `~someone`
/// is a filename that starts with a tilde, not another user's home.
pub fn resolve(base: &Path, home: &Path, input: &OsStr) -> Option<PathBuf> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let joined = if bytes == b"~" {
        home.to_path_buf()
    } else if let Some(rest) = bytes.strip_prefix(b"~/") {
        home.join(OsStr::from_bytes(rest))
    } else {
        // `join` on an absolute input replaces the base, which is what typing
        // an absolute path into the field should do.
        base.join(OsStr::from_bytes(bytes))
    };

    Some(fold(&joined))
}

fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What would happen if the name currently in the field were committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    /// Nothing typed.
    Empty,
    /// Names a directory, so there is no filename yet.
    IsDirectory(PathBuf),
    /// A path component that would have to be a directory is a file.
    Blocked { path: PathBuf, obstacle: PathBuf },
    /// Free. `parents` are the directories that do not exist yet, shallowest
    /// first — the order in which they would have to be created.
    New {
        path: PathBuf,
        parents: Vec<PathBuf>,
    },
    /// Something is already there and committing replaces it.
    Overwrite(PathBuf),
}

pub fn save_target(base: &Path, home: &Path, name: &OsStr) -> SaveTarget {
    let Some(path) = resolve(base, home, name) else {
        return SaveTarget::Empty;
    };

    // A trailing slash is the one case where intent is stated outright.
    if name.as_bytes().ends_with(b"/") {
        return SaveTarget::IsDirectory(path);
    }

    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => return SaveTarget::IsDirectory(path),
        Ok(_) => return SaveTarget::Overwrite(path),
        // A broken symlink stats as an error but is still something in the way.
        Err(_) if std::fs::symlink_metadata(&path).is_ok() => {
            return SaveTarget::Overwrite(path);
        }
        Err(_) => {}
    }

    let mut parents = Vec::new();
    let mut obstacle = None;
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        match std::fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                obstacle = Some(dir.to_path_buf());
                break;
            }
            Err(_) => {
                parents.push(dir.to_path_buf());
                ancestor = dir.parent();
            }
        }
    }

    match obstacle {
        Some(obstacle) => SaveTarget::Blocked { path, obstacle },
        None => {
            parents.reverse();
            SaveTarget::New { path, parents }
        }
    }
}

// ---------------------------------------------------------------- filters

/// Deciding whether a file is one of the kinds the application asked for.
///
/// A module of its own so the dead-code allow it needs — nothing calls any of
/// this until the filter UI exists — stays off the rest of the file.
#[allow(dead_code)]
pub mod filters {
    use crate::protocol::{Filter, FilterRule};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// Whether `name` — a bare filename, not a path — satisfies the filter.
    ///
    /// A filter with no rules matches nothing; an application that sends one is
    /// saying "none of these", not "all of these".
    pub fn matches(filter: &Filter, name: &OsStr) -> bool {
        filter.rules.iter().any(|rule| matches_rule(rule, name))
    }

    pub fn matches_rule(rule: &FilterRule, name: &OsStr) -> bool {
        match rule {
            FilterRule::Glob(pattern) => glob_match(pattern.as_bytes(), name.as_bytes()),
            FilterRule::MimeType(mime) => matches_mime(mime, name),
        }
    }

    fn matches_mime(rule: &str, name: &OsStr) -> bool {
        let Some((ty, sub)) = rule.split_once('/') else {
            return false;
        };
        // Guessed from the extension: the file may not exist, so sniffing content
        // is not on the table even when it would be more accurate.
        mime_guess::from_path(Path::new(name)).iter().any(|guess| {
            (ty == "*" || guess.type_().as_str().eq_ignore_ascii_case(ty))
                && (sub == "*" || guess.subtype().as_str().eq_ignore_ascii_case(sub))
        })
    }

    /// Matches `*`, `?` and `[…]` against bytes.
    ///
    /// Bytes rather than `str` because filenames are bytes, and glob patterns from
    /// applications are ASCII in practice. Writing this out avoids a glob crate's
    /// path semantics, which are wrong here: `*` must cross nothing, since what is
    /// being matched is a single filename.
    fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
        let (mut p, mut n) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;

        loop {
            if p < pattern.len() {
                match pattern[p] {
                    b'*' => {
                        p += 1;
                        star = Some((p, n));
                        continue;
                    }
                    b'?' if n < name.len() => {
                        p += 1;
                        n += 1;
                        continue;
                    }
                    b'[' if n < name.len() => match class_match(&pattern[p..], name[n]) {
                        Some((len, true)) => {
                            p += len;
                            n += 1;
                            continue;
                        }
                        Some((_, false)) => {}
                        // Unterminated: a bracket with no closer is a literal one.
                        None if name[n] == b'[' => {
                            p += 1;
                            n += 1;
                            continue;
                        }
                        None => {}
                    },
                    c if n < name.len() && c == name[n] => {
                        p += 1;
                        n += 1;
                        continue;
                    }
                    _ => {}
                }
            } else if n == name.len() {
                return true;
            }

            // Mismatch. Give the most recent `*` one more byte to swallow.
            match star {
                Some((resume, eaten)) if eaten < name.len() => {
                    p = resume;
                    n = eaten + 1;
                    star = Some((resume, n));
                }
                _ => return false,
            }
        }
    }

    /// Returns the class's length in bytes and whether it matched, or `None` if it
    /// is never closed.
    fn class_match(pattern: &[u8], byte: u8) -> Option<(usize, bool)> {
        let mut i = 1;
        let negated = matches!(pattern.get(i), Some(b'!' | b'^'));
        if negated {
            i += 1;
        }

        let mut hit = false;
        let mut first = true;
        loop {
            match pattern.get(i) {
                None => return None,
                // A `]` in the first position is a literal, per POSIX.
                Some(b']') if !first => {
                    i += 1;
                    break;
                }
                Some(&lo) => {
                    first = false;
                    // `-` immediately before the closing bracket is a literal.
                    if pattern.get(i + 1) == Some(&b'-')
                        && pattern.get(i + 2).is_some_and(|&c| c != b']')
                    {
                        hit |= (lo..=pattern[i + 2]).contains(&byte);
                        i += 3;
                    } else {
                        hit |= lo == byte;
                        i += 1;
                    }
                }
            }
        }

        Some((i, hit != negated))
    }

    /// The extension a filter implies, for pre-filling the Name field.
    ///
    /// Only patterns of the form `*.something` say anything useful; `*` and
    /// `*.[ch]?` do not, and guessing there would put a wrong extension on the
    /// file. A character class contributes its first member, so the
    /// `*.[jJ][pP][gG]` that GTK applications send yields `jpg`.
    pub fn extension(filter: &Filter) -> Option<String> {
        filter.rules.iter().find_map(|rule| match rule {
            FilterRule::Glob(pattern) => glob_extension(pattern.as_bytes()),
            FilterRule::MimeType(mime) => mime_guess::get_mime_extensions_str(mime)
                .and_then(|exts| exts.first())
                .map(|ext| (*ext).to_owned()),
        })
    }

    fn glob_extension(pattern: &[u8]) -> Option<String> {
        let rest = pattern.strip_prefix(b"*.")?;
        let mut ext = String::new();
        let mut i = 0;
        while i < rest.len() {
            match rest[i] {
                b'*' | b'?' => return None,
                b'[' => {
                    // Only for the length and to reject an unterminated class; the
                    // byte probed is irrelevant.
                    let (len, _) = class_match(&rest[i..], 0)?;
                    // A negated class stands for too many things to name one.
                    if matches!(rest.get(i + 1), Some(b'!' | b'^')) {
                        return None;
                    }
                    ext.push(*rest.get(i + 1)? as char);
                    i += len;
                }
                c => {
                    ext.push(c as char);
                    i += 1;
                }
            }
        }
        (!ext.is_empty()).then_some(ext)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn os(s: &str) -> &OsStr {
            OsStr::new(s)
        }

        fn glob(pattern: &str) -> Filter {
            Filter {
                label: pattern.to_owned(),
                rules: vec![FilterRule::Glob(pattern.to_owned())],
            }
        }

        // -- globs

        #[test]
        fn a_plain_extension_glob_matches_only_that_extension() {
            assert!(glob_match(b"*.png", b"photo.png"));
            assert!(!glob_match(b"*.png", b"photo.jpg"));
            assert!(!glob_match(b"*.png", b"png"));
            assert!(glob_match(b"*.png", b".png"));
        }

        #[test]
        fn globs_are_case_sensitive_which_is_why_applications_send_classes() {
            assert!(!glob_match(b"*.jpg", b"photo.JPG"));
            assert!(glob_match(b"*.[jJ][pP][gG]", b"photo.JPG"));
            assert!(glob_match(b"*.[jJ][pP][gG]", b"photo.jpg"));
            assert!(!glob_match(b"*.[jJ][pP][gG]", b"photo.png"));
        }

        #[test]
        fn a_star_in_the_middle_backtracks() {
            assert!(glob_match(b"a*b*.txt", b"axxbyy.txt"));
            assert!(!glob_match(b"a*b*.txt", b"axxyy.txt"));
            assert!(glob_match(b"*", b"anything at all"));
            assert!(glob_match(b"*", b""));
        }

        #[test]
        fn question_marks_take_exactly_one_byte() {
            assert!(glob_match(b"?.txt", b"a.txt"));
            assert!(!glob_match(b"?.txt", b"ab.txt"));
            assert!(!glob_match(b"?.txt", b".txt"));
        }

        #[test]
        fn classes_handle_ranges_and_negation() {
            assert!(glob_match(b"[a-z].txt", b"q.txt"));
            assert!(!glob_match(b"[a-z].txt", b"Q.txt"));
            assert!(glob_match(b"[!a-z].txt", b"Q.txt"));
            assert!(!glob_match(b"[!a-z].txt", b"q.txt"));
        }

        #[test]
        fn an_unclosed_bracket_is_a_literal_bracket() {
            assert!(glob_match(b"[abc", b"[abc"));
            assert!(!glob_match(b"[abc", b"a"));
        }

        #[test]
        fn a_filter_with_no_rules_matches_nothing() {
            let empty = Filter {
                label: "Nothing".into(),
                rules: vec![],
            };
            assert!(!matches(&empty, os("a.txt")));
        }

        #[test]
        fn any_one_rule_is_enough() {
            let filter = Filter {
                label: "Images".into(),
                rules: vec![
                    FilterRule::Glob("*.png".into()),
                    FilterRule::Glob("*.jpg".into()),
                ],
            };
            assert!(matches(&filter, os("a.jpg")));
            assert!(matches(&filter, os("a.png")));
            assert!(!matches(&filter, os("a.gif")));
        }

        // -- mime types

        #[test]
        fn mime_rules_are_guessed_from_the_extension() {
            let rule = FilterRule::MimeType("image/png".into());
            assert!(matches_rule(&rule, os("a.png")));
            assert!(!matches_rule(&rule, os("a.jpg")));
            assert!(!matches_rule(&rule, os("a")));
        }

        #[test]
        fn a_mime_wildcard_matches_the_whole_type() {
            let rule = FilterRule::MimeType("image/*".into());
            assert!(matches_rule(&rule, os("a.png")));
            assert!(matches_rule(&rule, os("a.jpg")));
            assert!(!matches_rule(&rule, os("a.txt")));
        }

        #[test]
        fn a_mime_rule_without_a_slash_matches_nothing() {
            let rule = FilterRule::MimeType("image".into());
            assert!(!matches_rule(&rule, os("a.png")));
        }

        // -- suggested extensions

        #[test]
        fn a_simple_glob_suggests_its_extension() {
            assert_eq!(
                extension(&glob("*.corkboard")).as_deref(),
                Some("corkboard")
            );
        }

        #[test]
        fn a_case_class_suggests_its_first_spelling() {
            assert_eq!(extension(&glob("*.[jJ][pP][gG]")).as_deref(), Some("jpg"));
        }

        #[test]
        fn a_glob_that_names_no_extension_suggests_nothing() {
            assert_eq!(extension(&glob("*")), None);
            assert_eq!(extension(&glob("*.??")), None);
            assert_eq!(extension(&glob("*.[!x]")), None);
        }

        #[test]
        fn a_mime_filter_suggests_an_extension_too() {
            let filter = Filter {
                label: "PNG".into(),
                rules: vec![FilterRule::MimeType("image/png".into())],
            };
            assert_eq!(extension(&filter).as_deref(), Some("png"));
        }
    }
}

// ---------------------------------------------------------------- matching

/// Fuzzy matching, holding the matcher's scratch buffers between calls.
pub struct Fuzzy {
    matcher: nucleo_matcher::Matcher,
    buf: Vec<char>,
}

impl Default for Fuzzy {
    fn default() -> Self {
        Self {
            matcher: nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
            buf: Vec::new(),
        }
    }
}

impl Fuzzy {
    /// Indices of the entries to show, in the order given.
    ///
    /// Filtering only, never reordering: a listing you already know keeps its
    /// shape while you type, and the row under the cursor does not jump out
    /// from under you. Ranking is worth its cost in the jump list, where the
    /// candidates are thousands of paths nobody has looked at.
    ///
    /// A query beginning with `.` reveals dotfiles without the toggle — the
    /// only way to type the name of one and see nothing would be worse.
    pub fn visible(&mut self, entries: &[Entry], query: &str, show_hidden: bool) -> Vec<usize> {
        let hidden_too = show_hidden || query.starts_with('.');
        let pattern = (!query.is_empty()).then(|| {
            nucleo_matcher::pattern::Pattern::parse(
                query,
                nucleo_matcher::pattern::CaseMatching::Smart,
                nucleo_matcher::pattern::Normalization::Smart,
            )
        });

        entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| hidden_too || !entry.hidden)
            .filter(|(_, entry)| match &pattern {
                None => true,
                Some(pattern) => self.score(pattern, &entry.name).is_some(),
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn score(&mut self, pattern: &nucleo_matcher::pattern::Pattern, name: &OsStr) -> Option<u32> {
        let Self { matcher, buf } = self;
        let text = name.to_string_lossy();
        pattern.score(nucleo_matcher::Utf32Str::new(&text, buf), matcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    // -- resolution

    #[test]
    fn a_bare_tilde_is_home() {
        let got = resolve(Path::new("/tmp"), Path::new("/home/me"), os("~"));
        assert_eq!(got, Some(PathBuf::from("/home/me")));
    }

    #[test]
    fn tilde_slash_expands() {
        let got = resolve(
            Path::new("/tmp"),
            Path::new("/home/me"),
            os("~/Pictures/a.png"),
        );
        assert_eq!(got, Some(PathBuf::from("/home/me/Pictures/a.png")));
    }

    #[test]
    fn a_tilde_with_a_name_after_it_is_just_a_filename() {
        let got = resolve(Path::new("/tmp"), Path::new("/home/me"), os("~root"));
        assert_eq!(got, Some(PathBuf::from("/tmp/~root")));
    }

    #[test]
    fn an_absolute_name_replaces_the_current_directory() {
        let got = resolve(Path::new("/tmp"), Path::new("/home/me"), os("/etc/hosts"));
        assert_eq!(got, Some(PathBuf::from("/etc/hosts")));
    }

    #[test]
    fn a_name_carrying_slashes_stays_relative_to_here() {
        let got = resolve(
            Path::new("/tmp/work"),
            Path::new("/home/me"),
            os("out/a.txt"),
        );
        assert_eq!(got, Some(PathBuf::from("/tmp/work/out/a.txt")));
    }

    #[test]
    fn dot_dot_is_folded_without_touching_the_disk() {
        let got = resolve(
            Path::new("/tmp/work"),
            Path::new("/home/me"),
            os("../elsewhere/./a.txt"),
        );
        assert_eq!(got, Some(PathBuf::from("/tmp/elsewhere/a.txt")));
    }

    #[test]
    fn dot_dot_cannot_climb_above_the_root() {
        let got = resolve(Path::new("/"), Path::new("/home/me"), os("../../etc"));
        assert_eq!(got, Some(PathBuf::from("/etc")));
    }

    #[test]
    fn an_empty_field_resolves_to_nothing() {
        assert_eq!(
            resolve(Path::new("/tmp"), Path::new("/home/me"), os("")),
            None
        );
    }

    // -- save targets

    #[test]
    fn an_empty_name_is_not_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let got = save_target(dir.path(), dir.path(), os(""));
        assert_eq!(got, SaveTarget::Empty);
    }

    #[test]
    fn a_free_name_in_an_existing_directory_creates_no_parents() {
        let dir = tempfile::tempdir().unwrap();
        let got = save_target(dir.path(), dir.path(), os("board.corkboard"));
        assert_eq!(
            got,
            SaveTarget::New {
                path: dir.path().join("board.corkboard"),
                parents: vec![],
            }
        );
    }

    #[test]
    fn nothing_is_created_on_disk_by_examining_a_target() {
        let dir = tempfile::tempdir().unwrap();
        save_target(dir.path(), dir.path(), os("deep/er/board.corkboard"));
        assert!(!dir.path().join("deep").exists());
    }

    #[test]
    fn missing_parents_come_back_in_creation_order() {
        let dir = tempfile::tempdir().unwrap();
        let got = save_target(dir.path(), dir.path(), os("deep/er/still/board.corkboard"));
        assert_eq!(
            got,
            SaveTarget::New {
                path: dir.path().join("deep/er/still/board.corkboard"),
                parents: vec![
                    dir.path().join("deep"),
                    dir.path().join("deep/er"),
                    dir.path().join("deep/er/still"),
                ],
            }
        );
    }

    #[test]
    fn an_existing_file_is_an_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let got = save_target(dir.path(), dir.path(), os("a.txt"));
        assert_eq!(got, SaveTarget::Overwrite(dir.path().join("a.txt")));
    }

    #[test]
    fn an_existing_directory_is_not_a_filename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let got = save_target(dir.path(), dir.path(), os("sub"));
        assert_eq!(got, SaveTarget::IsDirectory(dir.path().join("sub")));
    }

    #[test]
    fn a_trailing_slash_names_a_directory_even_where_there_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let got = save_target(dir.path(), dir.path(), os("nothing/here/"));
        assert_eq!(
            got,
            SaveTarget::IsDirectory(dir.path().join("nothing/here"))
        );
    }

    #[test]
    fn a_file_standing_where_a_directory_would_have_to_be_blocks_the_save() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let got = save_target(dir.path(), dir.path(), os("a.txt/b.txt"));
        assert_eq!(
            got,
            SaveTarget::Blocked {
                path: dir.path().join("a.txt/b.txt"),
                obstacle: dir.path().join("a.txt"),
            }
        );
    }

    // -- listing

    #[test]
    fn directories_come_first_and_case_does_not_split_the_alphabet() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["beta.txt", "Alpha.txt", ".hidden"] {
            std::fs::write(dir.path().join(name), b"1234").unwrap();
        }
        std::fs::create_dir(dir.path().join("zeta")).unwrap();

        let entries = list_dir(dir.path()).unwrap();
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["zeta", ".hidden", "Alpha.txt", "beta.txt"]);
    }

    #[test]
    fn a_symlink_to_a_directory_lists_as_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let entries = list_dir(dir.path()).unwrap();
        assert!(entries.iter().all(|e| e.kind == Kind::Dir), "{entries:?}");
    }

    #[test]
    fn a_broken_symlink_is_still_listed() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("link")).unwrap();

        let entries = list_dir(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, OsString::from("link"));
    }

    #[test]
    fn dotfiles_are_marked_rather_than_dropped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".config"), b"x").unwrap();
        std::fs::write(dir.path().join("config"), b"x").unwrap();

        let entries = list_dir(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().filter(|e| e.hidden).count() == 1);
    }

    #[test]
    fn sizes_read_like_a_gui_chooser() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 kB");
        assert_eq!(format_size(31_500), "31.5 kB");
        assert_eq!(format_size(1_200_000), "1.2 MB");
    }
}
