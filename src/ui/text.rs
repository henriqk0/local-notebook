//! Text helpers: sanitising untrusted content and display-width aware wrapping.
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Make untrusted text safe to draw: drops terminal control characters (a stray
/// ESC sequence in a file or model reply could otherwise drive the terminal),
/// bidi overrides and carriage returns, and expands tabs. Newlines are kept.
pub fn safe_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn format_tokens(count: usize) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Fit `s` into `max` display columns, ending in `...` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > keep {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push_str(&".".repeat(max.min(3)));
    out
}

/// Break a single over-long word into pieces no wider than `max`.
fn hard_break(word: &str, max: usize) -> Vec<String> {
    let mut pieces = vec![String::new()];
    let mut w = 0;
    for c in word.chars() {
        let cw = char_width(c);
        if w + cw > max && w > 0 {
            pieces.push(String::new());
            w = 0;
        }
        pieces.last_mut().expect("never empty").push(c);
        w += cw;
    }
    pieces
}

/// Word-wrap to `max` columns. Existing newlines and leading indentation are kept;
/// words wider than a line are broken. `max` below 1 is treated as 1.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let indent = raw.len() - raw.trim_start_matches(' ').len();
        let indent = indent.min(max / 2);
        let mut cur = " ".repeat(indent);
        let mut has_word = false;
        for word in raw
            .trim_start_matches(' ')
            .split(' ')
            .filter(|w| !w.is_empty())
        {
            let ww = width(word);
            if !has_word {
                if width(&cur) + ww <= max {
                    cur.push_str(word);
                } else {
                    let room = max - width(&cur);
                    // The first word doesn't fit after the indent: restart without it.
                    if room == 0 || ww > max {
                        if cur.trim().is_empty() {
                            cur.clear();
                        }
                        let mut pieces = hard_break(word, max);
                        let last = pieces.pop().unwrap_or_default();
                        lines.extend(pieces);
                        cur = last;
                    } else {
                        cur.push_str(word);
                    }
                }
                has_word = true;
            } else if width(&cur) + 1 + ww <= max {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(std::mem::take(&mut cur));
                let mut pieces = hard_break(word, max);
                cur = pieces.pop().unwrap_or_default();
                lines.extend(pieces);
            }
        }
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_text_strips_terminal_escapes_and_bidi() {
        assert_eq!(
            safe_text("a\x1b[31mred\x1b[0m\x07b\r\nc\td"),
            "a[31mred[0mb\nc    d"
        );
        assert_eq!(safe_text("x\u{202E}evil\u{9b}"), "xevil");
        assert_eq!(safe_text("héllo 日本"), "héllo 日本");
    }

    #[test]
    fn token_formatting() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1500), "1.5k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn truncate_by_display_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
        assert_eq!(truncate("日本語日本語", 7), "日本...");
        assert_eq!(truncate("abcdef", 2), "..");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn wrap_basic_newlines_and_long_words() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("ab abcdefgh", 4), ["ab", "abcd", "efgh"]);
    }

    #[test]
    fn wrap_keeps_indentation_and_never_exceeds_width() {
        assert_eq!(wrap("    code line", 20), ["    code line"]);
        let text = "The quick brown fox 日本語のテキスト jumps over the lazy dog ".repeat(5);
        for w in [1, 2, 5, 17, 40] {
            for line in wrap(&text, w) {
                assert!(width(&line) <= w.max(2), "{w}: {line:?}");
            }
        }
    }
}
