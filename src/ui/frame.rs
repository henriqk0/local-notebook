//! Box-drawing primitives. Panes are registered on a `LineGrid`, which records which
//! directions each cell's line connects to and picks the glyph at the end, so borders
//! that touch join into proper `┬ ┴ ├ ┤ ┼` junctions instead of sitting side by side.
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::text::{truncate, width};
use super::theme::Theme;

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn glyph(mask: u8) -> &'static str {
    match mask {
        m if m == UP | DOWN => "│",
        m if m == LEFT | RIGHT => "─",
        m if m == DOWN | RIGHT => "┌",
        m if m == DOWN | LEFT => "┐",
        m if m == UP | RIGHT => "└",
        m if m == UP | LEFT => "┘",
        m if m == UP | DOWN | RIGHT => "├",
        m if m == UP | DOWN | LEFT => "┤",
        m if m == LEFT | RIGHT | DOWN => "┬",
        m if m == LEFT | RIGHT | UP => "┴",
        m if m == UP | DOWN | LEFT | RIGHT => "┼",
        m if m & (UP | DOWN) != 0 && m & (LEFT | RIGHT) == 0 => "│",
        _ => "─",
    }
}

pub struct LineGrid {
    area: Rect,
    mask: Vec<u8>,
    focus: Vec<bool>,
}

impl LineGrid {
    pub fn new(area: Rect) -> Self {
        let n = area.width as usize * area.height as usize;
        Self {
            area,
            mask: vec![0; n],
            focus: vec![false; n],
        }
    }

    fn add(&mut self, x: u16, y: u16, m: u8, focused: bool) {
        if x < self.area.x || y < self.area.y || x >= self.area.right() || y >= self.area.bottom() {
            return;
        }
        let i = (y - self.area.y) as usize * self.area.width as usize + (x - self.area.x) as usize;
        self.mask[i] |= m;
        self.focus[i] |= focused;
    }

    /// The outline of `r`. Edges shared with another rect merge into junctions.
    pub fn rect(&mut self, r: Rect, focused: bool) {
        if r.width < 2 || r.height < 2 {
            return;
        }
        let (x1, y1) = (r.right() - 1, r.bottom() - 1);
        for x in r.x..=x1 {
            let l = if x > r.x { LEFT } else { 0 };
            let rt = if x < x1 { RIGHT } else { 0 };
            self.add(x, r.y, l | rt, focused);
            self.add(x, y1, l | rt, focused);
        }
        for y in r.y..=y1 {
            let u = if y > r.y { UP } else { 0 };
            let d = if y < y1 { DOWN } else { 0 };
            self.add(r.x, y, u | d, focused);
            self.add(x1, y, u | d, focused);
        }
    }

    /// A horizontal divider from `x0` to `x1` (inclusive) that ends in the borders on either side.
    pub fn hline(&mut self, x0: u16, x1: u16, y: u16, focused: bool) {
        for x in x0..=x1 {
            let l = if x > x0 { LEFT } else { 0 };
            let r = if x < x1 { RIGHT } else { 0 };
            self.add(x, y, l | r, focused);
        }
    }

    /// A vertical divider from `y0` to `y1` (inclusive).
    pub fn vline(&mut self, x: u16, y0: u16, y1: u16, focused: bool) {
        for y in y0..=y1 {
            let u = if y > y0 { UP } else { 0 };
            let d = if y < y1 { DOWN } else { 0 };
            self.add(x, y, u | d, focused);
        }
    }

    /// Draw the lines. Only the glyph and foreground are set, so a selected row's
    /// background stays continuous across a divider.
    pub fn render(&self, buf: &mut Buffer, theme: &Theme) {
        for (i, &m) in self.mask.iter().enumerate() {
            if m == 0 {
                continue;
            }
            let x = self.area.x + (i % self.area.width as usize) as u16;
            let y = self.area.y + (i / self.area.width as usize) as u16;
            let style = theme.border_style(self.focus[i]);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(glyph(m))
                    .set_fg(style.fg.unwrap_or_default())
                    .set_style(Style::default().add_modifier(style.add_modifier));
            }
        }
    }
}

/// `┤ Title ├` set into a horizontal border starting two cells in.
pub fn border_title(buf: &mut Buffer, r: Rect, title: &str, focused: bool, theme: &Theme) {
    if r.width < 8 {
        return;
    }
    let text = truncate(title, r.width as usize - 8);
    let border = theme.border_style(focused);
    let x = r.x + 2;
    buf.set_string(x, r.y, "┤", border);
    buf.set_string(x + 1, r.y, format!(" {text} "), theme.title_style(focused));
    buf.set_string(x + 3 + width(&text) as u16, r.y, "├", border);
}

/// A double-line dialog frame (`╔═╗ ║ ╚═╝`) with a `╣ Title ╠` caption and a dithered
/// drop shadow. Clears and fills the interior. Returns the usable inner rect.
pub fn double_box(buf: &mut Buffer, r: Rect, title: &str, theme: &Theme) -> Rect {
    let bounds = buf.area;
    // Shadow first, so the box paints over its overlap.
    let shadow = theme.dim_style();
    for y in (r.y + 1)..=r.bottom() {
        for x in r.right()..(r.right() + 2).min(bounds.right()) {
            if y < bounds.bottom() {
                buf.set_string(x, y, "▒", shadow);
            }
        }
    }
    if r.bottom() < bounds.bottom() {
        for x in (r.x + 2)..(r.right() + 2).min(bounds.right()) {
            buf.set_string(x, r.bottom(), "▒", shadow);
        }
    }
    if r.width < 2 || r.height < 2 {
        return r;
    }
    let (x1, y1) = (r.right() - 1, r.bottom() - 1);
    let border = theme.accent_style();
    let blank = " ".repeat(r.width as usize);
    for y in r.y..=y1 {
        buf.set_string(r.x, y, &blank, theme.base());
    }
    buf.set_string(
        r.x,
        r.y,
        format!("╔{}╗", "═".repeat(r.width as usize - 2)),
        border,
    );
    buf.set_string(
        r.x,
        y1,
        format!("╚{}╝", "═".repeat(r.width as usize - 2)),
        border,
    );
    for y in (r.y + 1)..y1 {
        buf.set_string(r.x, y, "║", border);
        buf.set_string(x1, y, "║", border);
    }
    if r.width >= 10 {
        let text = truncate(title, r.width as usize - 8);
        buf.set_string(r.x + 2, r.y, "╣", border);
        buf.set_string(r.x + 3, r.y, format!(" {text} "), theme.title_style(true));
        buf.set_string(r.x + 5 + width(&text) as u16, r.y, "╠", border);
    }
    Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2)
}

#[allow(clippy::too_many_arguments)] // geometry + the three scroll quantities + theme; a struct would only add noise
/// A shaded scrollbar drawn over a vertical border: `░` track, `█` thumb.
pub fn scrollbar(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    height: u16,
    total: usize,
    view: usize,
    offset: usize,
    theme: &Theme,
) {
    if height == 0 || view == 0 || total <= view {
        return;
    }
    let h = height as usize;
    let thumb = (h * view / total).max(1).min(h);
    let travel = h - thumb;
    let top = (travel * offset.min(total - view))
        .checked_div(total - view)
        .unwrap_or(0);
    for i in 0..h {
        let (sym, style) = if i >= top && i < top + thumb {
            ("█", theme.accent_style())
        } else {
            ("░", theme.color(theme.border))
        };
        buf.set_string(x, y + i as u16, sym, style);
    }
}

/// `████░░░░` showing `used / total`, `cells` wide. Over budget fills red.
pub fn meter(
    used: usize,
    total: usize,
    cells: usize,
    theme: &Theme,
    base: Style,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::text::Span;
    let ratio = if total == 0 {
        0.0
    } else {
        used as f64 / total as f64
    };
    let filled = ((ratio * cells as f64).round() as usize).min(cells);
    let color = if ratio > 1.0 {
        theme.red
    } else if ratio > 0.8 {
        theme.yellow
    } else {
        theme.green
    };
    vec![
        Span::styled("█".repeat(filled), base.fg(color)),
        Span::styled("░".repeat(cells - filled), base.fg(theme.border)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn render(grid: &LineGrid, area: Rect) -> Vec<String> {
        let mut buf = Buffer::empty(area);
        grid.render(&mut buf, &Theme::terminal());
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .replace(' ', ".")
            })
            .collect()
    }

    #[test]
    fn single_box() {
        let a = Rect::new(0, 0, 6, 4);
        let mut g = LineGrid::new(a);
        g.rect(a, false);
        assert_eq!(render(&g, a), ["┌────┐", "│....│", "│....│", "└────┘"]);
    }

    #[test]
    fn touching_panes_share_their_border_and_form_junctions() {
        let a = Rect::new(0, 0, 11, 6);
        let mut g = LineGrid::new(a);
        g.rect(Rect::new(0, 0, 5, 6), false); // left pane
        g.rect(Rect::new(4, 0, 7, 4), false); // right-top shares x = 4
        g.rect(Rect::new(4, 3, 7, 3), true); // right-bottom shares y = 3
        assert_eq!(
            render(&g, a),
            [
                "┌───┬─────┐",
                "│...│.....│",
                "│...│.....│",
                "│...├─────┤",
                "│...│.....│",
                "└───┴─────┘"
            ]
        );
    }

    #[test]
    fn dividers_cross_into_plus_junctions() {
        let a = Rect::new(0, 0, 9, 5);
        let mut g = LineGrid::new(a);
        g.rect(a, false);
        g.hline(0, 8, 2, false);
        g.vline(4, 0, 4, false);
        assert_eq!(
            render(&g, a),
            [
                "┌───┬───┐",
                "│...│...│",
                "├───┼───┤",
                "│...│...│",
                "└───┴───┘"
            ]
        );
    }

    #[test]
    fn focused_cells_use_the_accent_color_and_shared_edges_inherit_it() {
        let a = Rect::new(0, 0, 9, 3);
        let mut g = LineGrid::new(a);
        g.rect(Rect::new(0, 0, 5, 3), true);
        g.rect(Rect::new(4, 0, 5, 3), false);
        let mut buf = Buffer::empty(a);
        let t = Theme::terminal();
        g.render(&mut buf, &t);
        assert_eq!(buf[(0, 0)].fg, t.accent);
        assert_eq!(buf[(4, 1)].fg, t.accent, "shared edge");
        assert_eq!(buf[(8, 1)].fg, t.border);
        assert_ne!(t.accent, Color::Reset);
    }

    #[test]
    fn out_of_area_and_degenerate_rects_are_ignored() {
        let a = Rect::new(0, 0, 4, 3);
        let mut g = LineGrid::new(a);
        g.rect(Rect::new(0, 0, 40, 30), false);
        g.rect(Rect::new(0, 0, 1, 1), false);
        g.hline(0, 100, 50, false);
        assert_eq!(render(&g, a)[0], "┌───");
    }

    #[test]
    fn title_and_double_box() {
        let t = Theme::terminal();
        let a = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        let inner = double_box(&mut buf, a, "Hi", &t);
        assert_eq!(inner, Rect::new(1, 1, 18, 3));
        let row = |y: u16| {
            (0..24)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert_eq!(row(0), "╔═╣ Hi ╠═══════════╗    ");
        assert_eq!(row(2), "║                  ║▒▒  ");
        assert_eq!(row(4), "╚══════════════════╝▒▒  ");
        assert_eq!(row(5), "  ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  ");

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        border_title(&mut buf, Rect::new(0, 0, 20, 3), "Chat", false, &t);
        assert_eq!(
            (2..10)
                .map(|x| buf[(x, 0)].symbol().to_string())
                .collect::<String>(),
            "┤ Chat ├"
        );
    }

    #[test]
    fn scrollbar_thumb_tracks_the_offset_and_hides_when_not_needed() {
        let t = Theme::terminal();
        let col = |total, view, offset| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, 10));
            scrollbar(&mut buf, 0, 0, 10, total, view, offset, &t);
            (0..10)
                .map(|y| buf[(0, y)].symbol().to_string())
                .collect::<String>()
        };
        assert_eq!(col(10, 10, 0), "          ");
        assert_eq!(col(100, 10, 0), "█░░░░░░░░░");
        assert_eq!(col(100, 10, 90), "░░░░░░░░░█");
        assert!(col(20, 10, 5).contains('█') && col(20, 10, 5).contains('░'));
    }

    #[test]
    fn meter_colors_follow_usage() {
        let t = Theme::builtin_first();
        let spans = meter(50, 100, 10, &t, Style::default());
        assert_eq!(
            (spans[0].content.as_ref(), spans[1].content.as_ref()),
            ("█████", "░░░░░")
        );
        assert_eq!(spans[0].style.fg, Some(t.green));
        assert_eq!(
            meter(90, 100, 10, &t, Style::default())[0].style.fg,
            Some(t.yellow)
        );
        assert_eq!(
            meter(150, 100, 10, &t, Style::default())[0].style.fg,
            Some(t.red)
        );
        assert_eq!(
            meter(0, 0, 4, &t, Style::default())[1].content.as_ref(),
            "░░░░"
        );
    }
}
