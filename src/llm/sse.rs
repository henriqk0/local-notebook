//! Server-sent-events line parsing.

#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    /// Blank line, comment/keep-alive, or a field we don't use.
    Skip,
    /// The `[DONE]` sentinel.
    Done,
    /// The payload after `data:`.
    Data(String),
}

pub fn parse_line(raw: &str) -> SseLine {
    let line = raw.trim();
    let Some(payload) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    match payload.trim() {
        "[DONE]" => SseLine::Done,
        p => SseLine::Data(p.to_string()),
    }
}

/// Splits a byte stream into lines without ever buffering more than `max` bytes
/// of an unterminated line, so a hostile server can't exhaust memory.
pub struct LineBuffer {
    buf: Vec<u8>,
    max: usize,
}

#[derive(Debug)]
pub struct LineTooLong;

impl LineBuffer {
    pub fn new(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            max,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, LineTooLong> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                lines.push(self.take());
            } else {
                self.buf.push(b);
                if self.buf.len() > self.max {
                    return Err(LineTooLong);
                }
            }
        }
        Ok(lines)
    }

    /// The unterminated trailing line, if any.
    pub fn finish(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.buf)
            .trim_end_matches('\r')
            .to_string();
        self.buf.clear();
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        assert_eq!(parse_line(": keep-alive"), SseLine::Skip);
        assert_eq!(parse_line(""), SseLine::Skip);
        assert_eq!(parse_line("event: x"), SseLine::Skip);
        assert_eq!(
            parse_line("data: {\"a\": 1}"),
            SseLine::Data("{\"a\": 1}".into())
        );
        assert_eq!(parse_line("data:[DONE]"), SseLine::Done);
    }

    #[test]
    fn buffer_handles_split_chunks_crlf_and_limits() {
        let mut b = LineBuffer::new(16);
        assert!(b.push(b"data: a").unwrap().is_empty());
        assert_eq!(b.push(b"b\r\ndata: c\n").unwrap(), ["data: ab", "data: c"]);
        assert_eq!(b.push(b"tail").unwrap().len(), 0);
        assert_eq!(b.finish().as_deref(), Some("tail"));
        assert!(b.push(&[b'x'; 17]).is_err());
    }
}
