//! Turn a file on disk into a `Source` plus its chunks.
use std::path::{Path, PathBuf};

use super::{chunk, extract};
use crate::models::Source;
use crate::tokens;

pub const NO_TEXT_ERROR: &str = "No extractable text found (OCR is not supported)";

/// Expand a leading `~` the way a shell would.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path
        .strip_prefix("~/")
        .or(if path == "~" { Some("") } else { None })
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

pub fn build_source_from_path(notebook_id: i64, path: &Path) -> (Source, Vec<String>) {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let filename = resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut ex = extract::extract_text(&resolved);
    if ex.error.is_none() && ex.content.trim().is_empty() {
        ex.error = Some(NO_TEXT_ERROR.into());
    }
    let ok = ex.error.is_none();
    let chunks = if ok {
        chunk::chunk_text(&ex.content)
    } else {
        Vec::new()
    };
    let source = Source {
        notebook_id,
        path: resolved.to_string_lossy().into_owned(),
        filename,
        mime: ex.mime.map(String::from),
        ext: ex.ext,
        size: ex.size,
        token_estimate: tokens::estimate(&ex.content),
        content_text: ex.content,
        enabled: ok,
        status: if ok { "ok" } else { "error" }.into(),
        error: ex.error,
        ..Default::default()
    };
    (source, chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_source_has_chunks_and_error_source_is_disabled() {
        let d = tempfile::tempdir().unwrap();
        let good = d.path().join("a.txt");
        std::fs::write(&good, "some text here").unwrap();
        let (s, chunks) = build_source_from_path(7, &good);
        assert_eq!(
            (
                s.status.as_str(),
                s.enabled,
                s.notebook_id,
                s.filename.as_str()
            ),
            ("ok", true, 7, "a.txt")
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(s.token_estimate, 4);

        let empty = d.path().join("e.txt");
        std::fs::write(&empty, "   \n").unwrap();
        let (s, chunks) = build_source_from_path(7, &empty);
        assert_eq!(
            (s.status.as_str(), s.enabled, s.error.as_deref()),
            ("error", false, Some(NO_TEXT_ERROR))
        );
        assert!(chunks.is_empty());
    }

    #[test]
    fn missing_file_is_an_error_source() {
        let (s, _) = build_source_from_path(1, Path::new("/definitely/not/here.txt"));
        assert_eq!(s.status, "error");
    }
}
