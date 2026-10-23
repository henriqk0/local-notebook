//! Text extraction for the supported file types.
use std::fs::File;
use std::io::Read;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

pub const MAX_FILE_SIZE_MB: u64 = 20;
pub const MAX_FILE_SIZE_BYTES: u64 = MAX_FILE_SIZE_MB * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_PDF_PAGES: usize = 1000;
/// Extracted text is capped independently of file size: a small PDF, DOCX or HTML
/// file can expand to far more text than its on-disk size suggests.
pub const MAX_TEXT_CHARS: usize = 5_000_000;

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "json", "yaml", "yml", "csv", "tsv", "py", "js", "ts",
    "java", "cpp", "c", "h", "go", "rs", "toml", "ini", "sh",
];

pub struct Extracted {
    pub content: String,
    pub size: u64,
    pub ext: Option<String>,
    pub mime: Option<&'static str>,
    pub error: Option<String>,
}

impl Extracted {
    fn new(
        size: u64,
        ext: &str,
        mime: Option<&'static str>,
        result: Result<String, String>,
    ) -> Self {
        let (content, error) = match result {
            Ok(c) => (c, None),
            Err(e) => (String::new(), Some(e)),
        };
        Self {
            content,
            size,
            ext: (!ext.is_empty()).then(|| ext.to_string()),
            mime,
            error,
        }
    }
}

/// Read at most `limit` bytes, failing if the file is larger (this also closes
/// the gap between a size check and the read).
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("Failed to read file: {e}"))?;
    let mut buf = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read file: {e}"))?;
    if buf.len() as u64 > limit {
        return Err(format!("File larger than {MAX_FILE_SIZE_MB}MB"));
    }
    Ok(buf)
}

fn read_text(path: &Path) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&read_limited(path, MAX_FILE_SIZE_BYTES)?).into_owned())
}

fn read_html(path: &Path) -> Result<String, String> {
    let html = scraper::Html::parse_document(&read_text(path)?);
    let mut parts: Vec<&str> = Vec::new();
    for node in html.tree.nodes() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        let hidden = node.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|e| matches!(e.name(), "script" | "style"))
        });
        if !hidden {
            parts.push(text);
        }
    }
    Ok(parts.join("\n"))
}

fn read_pdf(path: &Path) -> Result<String, String> {
    let bytes = read_limited(path, MAX_FILE_SIZE_BYTES)?;
    // The PDF parsers panic on some malformed input; one bad file must not take the app down.
    std::panic::catch_unwind(|| {
        let doc = pdf_extract::Document::load_mem(&bytes)
            .map_err(|e| format!("Failed to read PDF: {e}"))?;
        if doc.get_pages().len() > MAX_PDF_PAGES {
            return Err(format!("PDF has more than {MAX_PDF_PAGES} pages"));
        }
        let pages = pdf_extract::extract_text_from_mem_by_pages(&bytes)
            .map_err(|e| format!("Failed to read PDF: {e}"))?;
        Ok(pages.join("\n\n"))
    })
    .unwrap_or_else(|_| Err("Failed to read PDF: malformed document".into()))
}

fn read_docx(path: &Path) -> Result<String, String> {
    let bytes = read_limited(path, MAX_FILE_SIZE_BYTES)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Failed to read DOCX: {e}"))?;
    let declared: u64 = (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size()))
        .sum();
    if declared > MAX_DECOMPRESSED_BYTES {
        return Err("DOCX expands beyond the decompressed size limit".into());
    }
    let entry = zip
        .by_name("word/document.xml")
        .map_err(|e| format!("Failed to read DOCX: {e}"))?;
    // `take` bounds what is really inflated even if the declared size lies.
    let mut xml = Vec::new();
    entry
        .take(MAX_DECOMPRESSED_BYTES + 1)
        .read_to_end(&mut xml)
        .map_err(|e| format!("Failed to read DOCX: {e}"))?;
    if xml.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err("DOCX expands beyond the decompressed size limit".into());
    }
    docx_paragraphs(&xml)
        .map(|p| p.join("\n\n"))
        .map_err(|e| format!("Failed to read DOCX: {e}"))
}

fn docx_paragraphs(xml: &[u8]) -> Result<Vec<String>, quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(e) => match e.name().as_ref() {
                "w:t" => in_text = true,
                "w:tab" => current.push('\t'),
                "w:br" => current.push('\n'),
                _ => {}
            },
            Event::Empty(e) => match e.name().as_ref() {
                "w:tab" => current.push('\t'),
                "w:br" => current.push('\n'),
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => {
                    if !current.trim().is_empty() {
                        paragraphs.push(std::mem::take(&mut current));
                    }
                    current.clear();
                }
                _ => {}
            },
            Event::Text(t) if in_text => current.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => {
                if let Ok(Some(ch)) = r.resolve_char_ref() {
                    current.push(ch);
                } else {
                    match &*r.xml10_content() {
                        "amp" => current.push('&'),
                        "lt" => current.push('<'),
                        "gt" => current.push('>'),
                        "quot" => current.push('"'),
                        "apos" => current.push('\''),
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(paragraphs)
}

fn extract_inner(path: &Path) -> Extracted {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    if size > MAX_FILE_SIZE_BYTES {
        return Extracted::new(
            size,
            &ext,
            None,
            Err(format!("File larger than {MAX_FILE_SIZE_MB}MB")),
        );
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        return Extracted::new(size, &ext, Some("text/plain"), read_text(path));
    }
    match ext.as_str() {
        "pdf" => Extracted::new(size, &ext, Some("application/pdf"), read_pdf(path)),
        "docx" => Extracted::new(
            size,
            &ext,
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            read_docx(path),
        ),
        "html" | "htm" | "xhtml" => Extracted::new(size, &ext, Some("text/html"), read_html(path)),
        _ => Extracted::new(
            size,
            &ext,
            None,
            Err("Unsupported or binary file type".into()),
        ),
    }
}

pub fn extract_text(path: &Path) -> Extracted {
    let mut out = extract_inner(path);
    if out.error.is_none() && out.content.chars().count() > MAX_TEXT_CHARS {
        out.content = String::new();
        out.error = Some(format!(
            "Extracted text exceeds {MAX_TEXT_CHARS} characters"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn make_docx(dir: &Path, body_xml: &str) -> std::path::PathBuf {
        let p = dir.join("a.docx");
        let mut z = zip::ZipWriter::new(File::create(&p).unwrap());
        z.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        z.write_all(format!("<w:document><w:body>{body_xml}</w:body></w:document>").as_bytes())
            .unwrap();
        z.finish().unwrap();
        p
    }

    #[test]
    fn text_html_and_unsupported() {
        let d = tempfile::tempdir().unwrap();
        let t = extract_text(&write(d.path(), "a.md", "héllo".as_bytes()));
        assert_eq!(
            (t.content.as_str(), t.error.is_none(), t.mime),
            ("héllo", true, Some("text/plain"))
        );

        let h = extract_text(&write(
            d.path(),
            "a.html",
            b"<html><script>evil()</script><style>p{}</style><p>Hi</p><p>there</p></html>",
        ));
        assert!(h.content.contains("Hi") && h.content.contains("there"));
        assert!(!h.content.contains("evil") && !h.content.contains("p{}"));

        let u = extract_text(&write(d.path(), "a.bin", b"\x00\x01"));
        assert_eq!(u.error.as_deref(), Some("Unsupported or binary file type"));
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let d = tempfile::tempdir().unwrap();
        let t = extract_text(&write(d.path(), "a.txt", b"ok \xff\xfe end"));
        assert!(t.error.is_none() && t.content.contains('\u{FFFD}'));
    }

    #[test]
    fn docx_paragraphs_and_entities() {
        let d = tempfile::tempdir().unwrap();
        let p = make_docx(
            d.path(),
            "<w:p><w:r><w:t>One &amp; two</w:t></w:r></w:p><w:p></w:p><w:p><w:r><w:t>Three</w:t></w:r></w:p>",
        );
        let e = extract_text(&p);
        assert_eq!(e.error, None);
        assert_eq!(e.content, "One & two\n\nThree");
    }

    #[test]
    fn broken_pdf_and_docx_become_errors_not_panics() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            extract_text(&write(d.path(), "a.pdf", b"%PDF-1.4 garbage"))
                .error
                .is_some()
        );
        assert!(
            extract_text(&write(d.path(), "b.docx", b"not a zip"))
                .error
                .is_some()
        );
    }

    #[test]
    fn oversized_text_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let big = "a".repeat(MAX_TEXT_CHARS + 1);
        let e = extract_text(&write(d.path(), "big.txt", big.as_bytes()));
        assert!(e.error.unwrap().contains("exceeds") && e.content.is_empty());
    }
}
