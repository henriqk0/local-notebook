//! A small text editing buffer for single- and multi-line inputs.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct TextBuf {
    chars: Vec<char>,
    cursor: usize,
    pub multiline: bool,
    pub mask: bool,
}

fn cw(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

impl TextBuf {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();
        Self {
            cursor: chars.len(),
            chars,
            multiline: false,
            mask: false,
        }
    }

    pub fn multiline() -> Self {
        Self {
            multiline: true,
            ..Default::default()
        }
    }

    pub fn masked() -> Self {
        Self {
            mask: true,
            ..Default::default()
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' && !self.multiline {
            return;
        }
        if c.is_control() && c != '\n' {
            return;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.replace("\r\n", "\n").chars() {
            self.insert_char(if c == '\r' {
                '\n'
            } else if c == '\t' {
                ' '
            } else {
                c
            });
        }
    }

    fn line_start(&self, i: usize) -> usize {
        self.chars[..i]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |p| p + 1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.chars[i..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.chars.len(), |p| i + p)
    }

    fn move_vertical(&mut self, down: bool) {
        let start = self.line_start(self.cursor);
        let col = self.cursor - start;
        if down {
            let end = self.line_end(self.cursor);
            if end >= self.chars.len() {
                self.cursor = self.chars.len();
                return;
            }
            let next_start = end + 1;
            self.cursor = (next_start + col).min(self.line_end(next_start));
        } else {
            if start == 0 {
                self.cursor = 0;
                return;
            }
            let prev_start = self.line_start(start - 1);
            self.cursor = (prev_start + col).min(start - 1);
        }
    }

    /// Apply an editing key. Returns whether the key was consumed. Enter and Esc are
    /// left to the caller.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if !ctrl && !alt => self.insert_char(c),
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(self.cursor),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(self.cursor),
            KeyCode::Char('u') if ctrl => {
                let start = self.line_start(self.cursor);
                self.chars.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char('k') if ctrl => {
                let end = self.line_end(self.cursor);
                self.chars.drain(self.cursor..end);
            }
            KeyCode::Char('w') if ctrl => {
                let mut i = self.cursor;
                while i > 0 && self.chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                while i > 0 && !self.chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                self.chars.drain(i..self.cursor);
                self.cursor = i;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = self.line_start(self.cursor),
            KeyCode::End => self.cursor = self.line_end(self.cursor),
            KeyCode::Up if self.multiline => self.move_vertical(false),
            KeyCode::Down if self.multiline => self.move_vertical(true),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    fn display_char(&self, c: char) -> char {
        if self.mask { '*' } else { c }
    }

    /// Hard-wrapped visual rows as `[start, end)` char ranges.
    fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let mut rows = Vec::new();
        let (mut start, mut w) = (0, 0);
        for (i, &c) in self.chars.iter().enumerate() {
            if c == '\n' {
                rows.push((start, i));
                start = i + 1;
                w = 0;
                continue;
            }
            let cwid = if self.mask { 1 } else { cw(c) };
            if w + cwid > width && i > start {
                rows.push((start, i));
                start = i;
                w = 0;
            }
            w += cwid;
        }
        rows.push((start, self.chars.len()));
        rows
    }

    fn row_text(&self, (s, e): (usize, usize)) -> String {
        self.chars[s..e]
            .iter()
            .map(|&c| self.display_char(c))
            .collect()
    }

    /// The visible rows (at most `height`, scrolled to keep the cursor in view) and
    /// the cursor position `(column, row)` within them.
    pub fn view(&self, width: usize, height: usize) -> (Vec<String>, (usize, usize)) {
        let height = height.max(1);
        if !self.multiline {
            return self.view_single(width);
        }
        let rows = self.rows(width);
        let cur_row = rows
            .iter()
            .rposition(|&(s, _)| s <= self.cursor)
            .unwrap_or(0);
        let col: usize = self.chars[rows[cur_row].0..self.cursor]
            .iter()
            .map(|&c| if self.mask { 1 } else { cw(c) })
            .sum();
        let top = (cur_row + 1).saturating_sub(height);
        let visible = rows[top..(top + height).min(rows.len())]
            .iter()
            .map(|&r| self.row_text(r))
            .collect();
        (visible, (col.min(width.saturating_sub(1)), cur_row - top))
    }

    fn view_single(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(2);
        // Scroll horizontally so the cursor (and one column of slack) stays visible.
        let mut start = self.cursor;
        let mut used = 0;
        while start > 0 {
            let w = if self.mask {
                1
            } else {
                cw(self.chars[start - 1])
            };
            if used + w >= width {
                break;
            }
            used += w;
            start -= 1;
        }
        let mut shown = String::new();
        let mut w = 0;
        for &c in &self.chars[start..] {
            let cwid = if self.mask { 1 } else { cw(c) };
            if w + cwid > width {
                break;
            }
            shown.push(self.display_char(c));
            w += cwid;
        }
        (vec![shown], (used, 0))
    }
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
    fn type_str(b: &mut TextBuf, s: &str) {
        for c in s.chars() {
            b.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn basic_editing() {
        let mut b = TextBuf::default();
        type_str(&mut b, "helo");
        b.handle_key(key(KeyCode::Left));
        b.handle_key(key(KeyCode::Char('l')));
        assert_eq!(b.text(), "hello");
        b.handle_key(key(KeyCode::Backspace));
        b.handle_key(key(KeyCode::Delete));
        assert_eq!(b.text(), "hel");
        b.handle_key(key(KeyCode::Home));
        b.handle_key(key(KeyCode::Delete));
        assert_eq!(b.text(), "el");
        assert!(!b.handle_key(key(KeyCode::Enter)) && !b.handle_key(key(KeyCode::Esc)));
    }

    #[test]
    fn single_line_rejects_newlines_and_controls() {
        let mut b = TextBuf::default();
        b.insert_str("a\nb\x1b[c\td");
        assert_eq!(b.text(), "ab[c d");
        assert_eq!(TextBuf::new("x\ny").text(), "xy");
    }

    #[test]
    fn kill_word_line_and_unicode() {
        let mut b = TextBuf::default();
        type_str(&mut b, "foo bar  ñandú");
        b.handle_key(ctrl('w'));
        assert_eq!(b.text(), "foo bar  ");
        b.handle_key(ctrl('w'));
        assert_eq!(b.text(), "foo ");
        b.handle_key(ctrl('u'));
        assert_eq!(b.text(), "");
    }

    #[test]
    fn multiline_navigation() {
        let mut b = TextBuf::multiline();
        b.insert_str("abc\nde\nfghij");
        b.handle_key(key(KeyCode::Up));
        b.handle_key(key(KeyCode::Up));
        b.handle_key(key(KeyCode::Up)); // clamps at the start
        b.handle_key(key(KeyCode::Char('X')));
        assert_eq!(b.text(), "Xabc\nde\nfghij");
        b.handle_key(key(KeyCode::Down));
        b.handle_key(key(KeyCode::Down));
        b.handle_key(key(KeyCode::Down)); // clamps at the end
        b.handle_key(key(KeyCode::Char('!')));
        assert!(b.text().ends_with("fghij!"));
    }

    #[test]
    fn view_wraps_scrolls_and_tracks_cursor() {
        let mut b = TextBuf::multiline();
        b.insert_str("aaaaaaaaaa\nbb");
        let (rows, (col, row)) = b.view(4, 10);
        assert_eq!(rows, ["aaaa", "aaaa", "aa", "bb"]);
        assert_eq!((col, row), (2, 3));
        let (rows, (_, row)) = b.view(4, 2);
        assert_eq!((rows.len(), row), (2, 1));
        assert_eq!(rows, ["aa", "bb"]);
        b.insert_str("\n");
        assert_eq!(b.view(4, 10).1, (0, 4));
    }

    #[test]
    fn masked_and_single_line_views() {
        let mut b = TextBuf::masked();
        b.insert_str("secret");
        assert_eq!(b.view(20, 1).0, ["******"]);
        let mut s = TextBuf::default();
        s.insert_str("0123456789");
        let (rows, (col, _)) = s.view(5, 1);
        assert_eq!(rows[0], "6789");
        assert_eq!(col, 4);
    }
}
