//! Small shared drawing helpers.
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::text::truncate;
use super::theme::Theme;

/// A centered rectangle of at most `w` x `h`, shrunk to fit inside `area`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// One-line key hints (`key label`), dropping whatever doesn't fit in `width`.
pub fn hints(pairs: &[(&str, &str)], width: usize, theme: &Theme) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ", theme.strip())];
    let mut used = 1;
    for (key, label) in pairs {
        let need = key.chars().count() + label.chars().count() + 3;
        if used + need > width {
            break;
        }
        spans.push(Span::styled(key.to_string(), theme.key_style()));
        spans.push(Span::styled(format!(" {label}  "), theme.strip()));
        used += need;
    }
    Line::from(spans)
}

pub fn fit(s: &str, width: usize) -> String {
    truncate(s, width)
}
