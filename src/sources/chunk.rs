//! Paragraph-aware chunking with a character overlap between chunks.
use crate::tokens::CHARS_PER_TOKEN;

pub const CHUNK_TOKENS: usize = 800;
pub const OVERLAP_TOKENS: usize = 100;

pub fn chunk_text(text: &str) -> Vec<String> {
    chunk_with(text, CHUNK_TOKENS, OVERLAP_TOKENS)
}

pub fn chunk_with(text: &str, chunk_tokens: usize, overlap_tokens: usize) -> Vec<String> {
    assert!(chunk_tokens > 0, "chunk_tokens must be positive");
    let chunk_chars = chunk_tokens * CHARS_PER_TOKEN;
    let overlap_chars = (overlap_tokens * CHARS_PER_TOKEN).min(chunk_chars / 2);

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let para_len = para.chars().count();

        if para_len > chunk_chars {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = para.chars().collect();
            let step = chunk_chars.saturating_sub(overlap_chars).max(1);
            for start in (0..chars.len()).step_by(step) {
                chunks.push(
                    chars[start..(start + chunk_chars).min(chars.len())]
                        .iter()
                        .collect(),
                );
            }
            continue;
        }

        if !current.is_empty() && current.chars().count() + para_len + 2 > chunk_chars {
            let previous = std::mem::take(&mut current);
            current = if overlap_chars > 0 {
                let skip = previous.chars().count().saturating_sub(overlap_chars);
                let tail: String = previous.chars().skip(skip).collect();
                format!("{tail}\n\n{para}")
            } else {
                para.to_string()
            };
            chunks.push(previous);
        } else if current.is_empty() {
            current = para.to_string();
        } else {
            current.push_str("\n\n");
            current.push_str(para);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_small() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("\n\n  \n\n").is_empty());
        assert_eq!(chunk_text("hello\n\nworld"), ["hello\n\nworld"]);
    }

    #[test]
    fn splits_on_paragraph_boundaries_with_overlap() {
        let para = "x".repeat(60);
        let text = format!("{para}\n\n{para}\n\n{para}");
        // chunk = 10 tokens = 40 chars, overlap = 2 tokens = 8 chars
        let c = chunk_with(&text, 20, 2); // 80 chars per chunk
        assert_eq!(c.len(), 3);
        assert!(c[1].starts_with(&"x".repeat(8)));
    }

    #[test]
    fn oversized_paragraph_is_windowed_and_covers_everything() {
        let text: String = ('a'..='z').cycle().take(1000).collect();
        let c = chunk_with(&text, 50, 10); // 200 chars, overlap 40
        assert!(c.iter().all(|p| p.chars().count() <= 200));
        assert!(c[0].chars().eq(text.chars().take(200)));
        assert_eq!(c[1].chars().next(), text.chars().nth(160));
        assert!(text.ends_with(c.last().unwrap().as_str()));
    }

    #[test]
    fn overlap_is_capped_at_half_a_chunk() {
        let text = "y".repeat(500);
        let c = chunk_with(&text, 10, 1000); // would loop forever without the cap
        assert!(c.len() > 1 && c.len() < 100);
    }

    #[test]
    #[should_panic]
    fn zero_chunk_size_is_rejected() {
        chunk_with("a", 0, 0);
    }
}
