//! The terminal half.
//!
//! Stub for now: reads the request, writes a fixed answer, exits. It exists so
//! the D-Bus chain can be proven end to end before any interface is drawn — the
//! integration is where the surprises live, and finding one here costs minutes
//! rather than a week of suspecting the wrong layer.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::protocol::{Mode, Request, Response, WirePath};

pub fn run(request_path: &Path, response_path: &Path) -> Result<()> {
    let raw = std::fs::read(request_path)
        .with_context(|| format!("reading {}", request_path.display()))?;
    let request: Request = serde_json::from_slice(&raw).context("parsing the request")?;

    let response = stub_answer(&request);

    std::fs::write(response_path, serde_json::to_vec(&response)?)
        .with_context(|| format!("writing {}", response_path.display()))
}

/// Answers whatever was asked without asking anyone.
///
/// `FILEPICK_STUB_CANCEL` makes it decline instead, so the cancel path is
/// reachable from a test too.
fn stub_answer(request: &Request) -> Response {
    if std::env::var_os("FILEPICK_STUB_CANCEL").is_some() {
        return Response::Cancelled;
    }

    let dir: PathBuf = request.start_dir.to_path_buf();
    let paths = match &request.mode {
        Mode::OpenDirectory { .. } => vec![WirePath::from(&dir)],
        Mode::SaveFiles { names } => names
            .iter()
            .map(|n| WirePath::from(dir.join(n.to_path_buf())))
            .collect(),
        Mode::OpenFile { .. } | Mode::SaveFile => {
            let name = request.current_name.as_deref().unwrap_or("filepick-stub");
            vec![WirePath::from(dir.join(name))]
        }
    };

    Response::Selected {
        paths,
        filter: request.current_filter,
    }
}
