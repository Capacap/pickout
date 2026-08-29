//! The daemon/dialog contract.
//!
//! The two halves of filepick are the same binary in different modes, and
//! everything they say to each other passes through these types as JSON in a
//! file under `$XDG_RUNTIME_DIR`. That is deliberate: the setup this replaces
//! used positional argv plus a bare temp file, an undocumented shape that had
//! to be re-derived from the wrapper script every time. The file was never the
//! problem; the unnamed contract was.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A path as it crosses the wire.
///
/// Unix paths are bytes, not text, and `serde_json` refuses to serialize a
/// `PathBuf` that is not UTF-8 — which would turn a file with an odd name into
/// a dialog that fails for no stated reason. Untagged, so the common case is a
/// plain readable string in the JSON and only genuinely non-UTF-8 paths fall
/// back to a byte array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WirePath {
    Text(String),
    Bytes(Vec<u8>),
}

impl WirePath {
    pub fn to_path_buf(&self) -> PathBuf {
        match self {
            Self::Text(s) => PathBuf::from(s),
            Self::Bytes(b) => PathBuf::from(OsString::from_vec(b.clone())),
        }
    }
}

impl<T: AsRef<Path>> From<T> for WirePath {
    fn from(path: T) -> Self {
        let bytes = path.as_ref().as_os_str().as_bytes();
        match std::str::from_utf8(bytes) {
            Ok(s) => Self::Text(s.to_owned()),
            Err(_) => Self::Bytes(bytes.to_vec()),
        }
    }
}

/// Which of the four questions the dialog is being asked.
///
/// `directory` arrives as a flag on `OpenFile` rather than as its own portal
/// method, but it is a different interaction and the UI branches on it, so it
/// gets its own variant here. `SaveFiles` likewise: the application supplies
/// the names, so the only question left is which directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    OpenFile { multiple: bool },
    OpenDirectory { multiple: bool },
    SaveFile,
    SaveFiles { names: Vec<WirePath> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterRule {
    Glob(String),
    MimeType(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub label: String,
    pub rules: Vec<FilterRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub mode: Mode,
    pub title: String,
    pub accept_label: Option<String>,
    /// Where the browser opens. Resolved by the daemon and guaranteed to be an
    /// existing directory, so the dialog never has to decide what to do with an
    /// application's nonsense suggestion.
    pub start_dir: WirePath,
    /// Pre-fill for the Name field; save modes only.
    pub current_name: Option<String>,
    pub filters: Vec<Filter>,
    /// Index into `filters`.
    pub current_filter: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Response {
    /// The paths chosen. In save modes these do not exist yet, and must not:
    /// a save dialog answers where to write, and the application does the
    /// writing. Creating them here is what littered the home directory with
    /// stub files under the setup this replaces.
    Selected {
        paths: Vec<WirePath>,
        /// Index into the request's `filters`.
        filter: Option<usize>,
    },
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_paths_stay_readable_in_json() {
        let json = serde_json::to_string(&WirePath::from("/home/me/a b.txt")).unwrap();
        assert_eq!(json, r#""/home/me/a b.txt""#);
    }

    #[test]
    fn non_utf8_paths_survive_the_round_trip() {
        let raw = PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe.bin".to_vec()));
        let wire = WirePath::from(&raw);
        assert!(matches!(wire, WirePath::Bytes(_)));
        let json = serde_json::to_string(&wire).unwrap();
        let back: WirePath = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_path_buf(), raw);
    }

    #[test]
    fn request_round_trips() {
        let req = Request {
            mode: Mode::SaveFile,
            title: "Save board".into(),
            accept_label: None,
            start_dir: WirePath::from("/home/me/Pictures"),
            current_name: Some("untitled.corkboard".into()),
            filters: vec![Filter {
                label: "Corkboard".into(),
                rules: vec![FilterRule::Glob("*.corkboard".into())],
            }],
            current_filter: Some(0),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
    }

    #[test]
    fn cancelled_response_round_trips() {
        let json = serde_json::to_string(&Response::Cancelled).unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&json).unwrap(),
            Response::Cancelled
        );
    }
}
