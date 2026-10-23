//! Markdown for the chat: turns a model's reply into styled, pre-wrapped terminal lines.
//!
//! Everything that reaches the screen goes through `clean` first, *after* parsing, because
//! Markdown entities (`&#27;`) can decode to control characters that a pre-sanitized
//! input would not contain. Output lines never exceed the requested width.
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::text::{safe_text, truncate, width};
use super::theme::Theme;

type Piece = (String, Style);

/// Inline content before wrapping: unbreakable words (possibly several styled pieces,
/// like `**bold**,`), breakable spaces, and hard line breaks.
#[derive(Clone, Debug)]
enum Tok {
    Word(Vec<Piece>),
    Space,
    Newline,
}

fn clean(s: &str) -> String {
    safe_text(s)
}

fn pieces_width(p: &[Piece]) -> usize {
    p.iter().map(|(t, _)| width(t)).sum()
}

fn append(line: &mut Vec<Piece>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some((t, s)) if *s == style => t.push_str(text),
        _ => line.push((text.to_string(), style)),
    }
}

/// Split a word wider than `max` into lines of at most `max` columns.
fn hard_break(word: &[Piece], max: usize) -> Vec<Vec<Piece>> {
    let max = max.max(1);
    let mut lines: Vec<Vec<Piece>> = vec![Vec::new()];
    let mut w = 0;
    for (text, style) in word {
        for c in text.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if w + cw > max && w > 0 {
                lines.push(Vec::new());
                w = 0;
            }
            let mut buf = [0u8; 4];
            append(
                lines.last_mut().expect("never empty"),
                c.encode_utf8(&mut buf),
                *style,
            );
            w += cw;
        }
    }
    lines
}

/// Greedy word wrap of styled tokens to `max` columns.
fn wrap_toks(toks: &[Tok], max: usize) -> Vec<Vec<Piece>> {
    let max = max.max(1);
    let mut lines: Vec<Vec<Piece>> = Vec::new();
    let (mut cur, mut cur_w, mut space) = (Vec::<Piece>::new(), 0usize, false);
    for t in toks {
        match t {
            Tok::Space => space = cur_w > 0,
            Tok::Newline => {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                space = false;
            }
            Tok::Word(w) => {
                let ww = pieces_width(w);
                let gap = usize::from(cur_w > 0 && space);
                if cur_w > 0 && cur_w + gap + ww <= max {
                    if gap == 1 {
                        append(&mut cur, " ", Style::default());
                    }
                    for (t, s) in w {
                        append(&mut cur, t, *s);
                    }
                    cur_w += gap + ww;
                } else {
                    if cur_w > 0 {
                        lines.push(std::mem::take(&mut cur));
                    }
                    if ww <= max {
                        cur = w.clone();
                        cur_w = ww;
                    } else {
                        let mut parts = hard_break(w, max);
                        let last = parts.pop().unwrap_or_default();
                        lines.extend(parts);
                        cur_w = pieces_width(&last);
                        cur = last;
                    }
                }
                space = false;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Hard-wrap plain text by display width (code lines must not be re-flowed).
fn chunk_by_width(s: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let (mut out, mut cur, mut w) = (Vec::new(), String::new(), 0);
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max && w > 0 {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += cw;
    }
    out.push(cur);
    out
}

enum Ctx {
    Quote,
    Item {
        marker: Option<String>,
        width: usize,
    },
}

struct ListState {
    next: Option<u64>,
}

struct CodeState {
    lang: String,
    text: String,
}

struct LinkState {
    url: String,
    text: String,
}

struct TableState {
    aligns: Vec<Alignment>,
    head: Vec<Vec<Tok>>,
    rows: Vec<Vec<Vec<Tok>>>,
    cur_row: Vec<Vec<Tok>>,
}

struct Builder<'t> {
    theme: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    last_blank: bool,
    toks: Vec<Tok>,
    cur: Vec<Piece>,
    styles: Vec<Style>,
    ctx: Vec<Ctx>,
    lists: Vec<ListState>,
    code: Option<CodeState>,
    links: Vec<LinkState>,
    table: Option<TableState>,
    heading: Option<HeadingLevel>,
}

impl<'t> Builder<'t> {
    fn new(theme: &'t Theme, width: usize) -> Self {
        Self {
            theme,
            width,
            out: Vec::new(),
            last_blank: true,
            toks: Vec::new(),
            cur: Vec::new(),
            styles: vec![theme.base()],
            ctx: Vec::new(),
            lists: Vec::new(),
            code: None,
            links: Vec::new(),
            table: None,
            heading: None,
        }
    }

    // ---- styles ---------------------------------------------------------------------

    fn style(&self) -> Style {
        *self.styles.last().expect("base style is never popped")
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn dim(&self) -> Style {
        self.theme.dim_style()
    }

    fn code_style(&self) -> Style {
        Style::default().fg(self.theme.yellow).bg(self.theme.sel_bg)
    }

    // ---- inline collection ----------------------------------------------------------

    fn finalize_word(&mut self) {
        if !self.cur.is_empty() {
            self.toks.push(Tok::Word(std::mem::take(&mut self.cur)));
        }
    }

    fn space(&mut self) {
        self.finalize_word();
        if !matches!(self.toks.last(), Some(Tok::Space) | None) {
            self.toks.push(Tok::Space);
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        let mut word = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                append(&mut self.cur, &word, style);
                word.clear();
                self.space();
            } else {
                word.push(c);
            }
        }
        append(&mut self.cur, &word, style);
    }

    /// Text that must not be split at spaces (inline code, `(url)`).
    fn push_atomic(&mut self, text: &str, style: Style) {
        append(&mut self.cur, text, style);
    }

    // ---- block layout ---------------------------------------------------------------

    /// The prefix (quote bars, list markers / indentation) for the next line. Spans
    /// are dropped from the outside in when nesting would leave no room for text.
    fn prefix(&mut self, consume: bool) -> Vec<Span<'static>> {
        let (dim, marker_style) = (self.dim(), self.theme.accent_style());
        let mut spans: Vec<Span<'static>> = Vec::new();
        for c in &mut self.ctx {
            match c {
                Ctx::Quote => spans.push(Span::styled("│ ", dim)),
                Ctx::Item { marker, width } => match marker {
                    Some(m) if consume => spans.push(Span::styled(
                        marker.take().unwrap_or_default(),
                        marker_style,
                    )),
                    Some(m) => spans.push(Span::styled(m.clone(), marker_style)),
                    None => spans.push(Span::raw(" ".repeat(*width))),
                },
            }
        }
        let limit = self.width.saturating_sub(4);
        let mut total: usize = spans.iter().map(|s| width(&s.content)).sum();
        let mut drop = 0;
        while total > limit && drop < spans.len() {
            total -= width(&spans[drop].content);
            drop += 1;
        }
        spans.drain(..drop);
        spans
    }

    fn prefix_width(&mut self) -> usize {
        self.prefix(false).iter().map(|s| width(&s.content)).sum()
    }

    fn avail(&mut self) -> usize {
        self.width.saturating_sub(self.prefix_width()).max(1)
    }

    fn emit(&mut self, pieces: Vec<Piece>) {
        let mut spans = self.prefix(true);
        spans.extend(pieces.into_iter().map(|(t, s)| Span::styled(t, s)));
        self.out.push(Line::from(spans));
        self.last_blank = false;
    }

    /// A blank line between blocks (carrying quote bars), except where nothing precedes it.
    fn gap(&mut self) {
        let nothing_emitted_in_item = self.ctx.iter().any(|c| {
            matches!(
                c,
                Ctx::Item {
                    marker: Some(_),
                    ..
                }
            )
        });
        if self.out.is_empty() || self.last_blank || nothing_emitted_in_item {
            return;
        }
        let mut spans = Vec::new();
        for c in &self.ctx {
            match c {
                Ctx::Quote => spans.push(Span::styled("│", self.dim())),
                Ctx::Item { width, .. } => spans.push(Span::raw(" ".repeat(*width))),
            }
        }
        self.out.push(Line::from(spans));
        self.last_blank = true;
    }

    /// Wrap and emit the pending inline content. Returns the widest line's text width.
    fn flush(&mut self) -> usize {
        self.finalize_word();
        if self.toks.is_empty() {
            return 0;
        }
        let toks = std::mem::take(&mut self.toks);
        let avail = self.avail();
        let mut widest = 0;
        for line in wrap_toks(&toks, avail) {
            widest = widest.max(pieces_width(&line));
            self.emit(line);
        }
        widest
    }

    // ---- events ---------------------------------------------------------------------

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                let t = clean(&t);
                if let Some(code) = self.code.as_mut() {
                    code.text.push_str(&t);
                } else {
                    if let Some(l) = self.links.last_mut() {
                        l.text.push_str(&t);
                    }
                    let style = self.style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(c) | Event::InlineMath(c) | Event::DisplayMath(c) => {
                let c = clean(&c).replace('\n', " ");
                if let Some(l) = self.links.last_mut() {
                    l.text.push_str(&c);
                }
                let style = self.code_style();
                self.push_atomic(&c, style);
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let style = self.dim();
                self.push_text(&clean(&h), style);
            }
            Event::SoftBreak => self.space(),
            Event::HardBreak => {
                self.finalize_word();
                self.toks.push(Tok::Newline);
            }
            Event::Rule => {
                self.flush();
                self.gap();
                let avail = self.avail();
                let style = self.dim();
                self.emit(vec![("─".repeat(avail), style)]);
            }
            Event::TaskListMarker(done) => {
                let style = self.theme.accent_style();
                self.push_atomic(if done { "[x]" } else { "[ ]" }, style);
                self.space();
            }
            Event::FootnoteReference(l) => {
                let style = self.dim();
                self.push_text(&format!("[^{}]", clean(&l)), style);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush();
                self.gap();
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.gap();
                self.heading = Some(level);
                let t = self.theme;
                let color = match level {
                    HeadingLevel::H1 => t.accent,
                    HeadingLevel::H2 => t.accent2,
                    _ => t.fg,
                };
                self.push_style(|s| s.fg(color).add_modifier(Modifier::BOLD));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.gap();
                self.ctx.push(Ctx::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.gap();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        clean(info.split_whitespace().next().unwrap_or("")).replace('\n', "")
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeState {
                    lang,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.flush();
                self.gap();
                let dim = self.dim();
                self.push_style(|_| dim);
            }
            Tag::List(first) => {
                self.flush();
                if !self.ctx.iter().any(|c| matches!(c, Ctx::Item { .. })) {
                    self.gap();
                }
                self.lists.push(ListState { next: first });
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(ListState { next: Some(n) }) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => ["• ", "○ ", "■ "][depth.saturating_sub(1).min(2)].to_string(),
                };
                let w = width(&marker);
                self.ctx.push(Ctx::Item {
                    marker: Some(marker),
                    width: w,
                });
            }
            Tag::Table(aligns) => {
                self.flush();
                self.gap();
                self.table = Some(TableState {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    cur_row: Vec::new(),
                });
            }
            Tag::TableHead => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.links.push(LinkState {
                    url: clean(&dest_url).replace('\n', ""),
                    text: String::new(),
                });
                let accent = self.theme.accent;
                self.push_style(|s| s.fg(accent).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { .. } => {
                let dim = self.dim();
                self.push_text("[image:", dim);
                self.space();
                self.push_style(|_| dim);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
            }
            TagEnd::Heading(level) => {
                let widest = self.flush();
                if let Some(rule) = match level {
                    HeadingLevel::H1 => Some('═'),
                    HeadingLevel::H2 => Some('─'),
                    _ => None,
                } {
                    let style = self.style();
                    let n = widest.min(self.avail());
                    if n > 0 {
                        self.emit(vec![(rule.to_string().repeat(n), style)]);
                    }
                }
                self.heading = None;
                self.pop_style();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                if matches!(self.ctx.last(), Some(Ctx::Quote)) {
                    self.ctx.pop();
                }
            }
            TagEnd::CodeBlock => self.render_code(),
            TagEnd::HtmlBlock => {
                self.flush();
                self.pop_style();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                if matches!(self.ctx.last(), Some(Ctx::Item { .. })) {
                    self.ctx.pop();
                }
            }
            TagEnd::Table => self.render_table(),
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.head = std::mem::take(&mut t.cur_row);
                }
                self.pop_style();
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.cur_row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                self.finalize_word();
                let cell = std::mem::take(&mut self.toks);
                if let Some(t) = self.table.as_mut() {
                    t.cur_row.push(cell);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some(l) = self.links.pop()
                    && !l.url.is_empty()
                    && l.text.trim() != l.url
                {
                    let dim = self.dim();
                    self.space();
                    self.push_atomic(&format!("({})", truncate(&l.url, 200)), dim);
                }
            }
            TagEnd::Image => {
                self.pop_style();
                let dim = self.dim();
                self.push_text("]", dim);
            }
            _ => {}
        }
    }

    // ---- code blocks and tables -----------------------------------------------------

    fn render_code(&mut self) {
        let Some(code) = self.code.take() else { return };
        let text = code.text.trim_end_matches('\n').to_string();
        let avail = self.avail();
        let (border, body) = (
            self.theme.border_style(false),
            Style::default().fg(self.theme.fg).bg(self.theme.sel_bg),
        );
        if avail < 8 {
            for line in text.split('\n') {
                for chunk in chunk_by_width(line, avail) {
                    self.emit(vec![(chunk, body)]);
                }
            }
            return;
        }
        let inner = avail - 4;
        let top = if code.lang.is_empty() {
            format!("┌{}┐", "─".repeat(avail - 2))
        } else {
            let label = format!("┤ {} ├", truncate(&code.lang, avail.saturating_sub(8)));
            format!(
                "┌─{label}{}┐",
                "─".repeat((avail - 3).saturating_sub(width(&label)))
            )
        };
        self.emit(vec![(top, border)]);
        for line in text.split('\n') {
            for chunk in chunk_by_width(line, inner) {
                let pad = " ".repeat(inner.saturating_sub(width(&chunk)));
                self.emit(vec![
                    ("│ ".into(), border),
                    (format!("{chunk}{pad}"), body),
                    (" │".into(), border),
                ]);
            }
        }
        self.emit(vec![(format!("└{}┘", "─".repeat(avail - 2)), border)]);
    }

    fn render_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let ncols = t
            .aligns
            .len()
            .max(t.head.len())
            .max(t.rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1);
        let pad_row = |mut r: Vec<Vec<Tok>>| {
            r.resize(ncols, Vec::new());
            r
        };
        let head = pad_row(t.head);
        let rows: Vec<_> = t.rows.into_iter().map(pad_row).collect();

        let natural = |cell: &[Tok]| -> usize {
            let mut w = 0;
            let mut gap = false;
            for tok in cell {
                match tok {
                    Tok::Word(p) => {
                        w += usize::from(gap && w > 0) + pieces_width(p);
                        gap = false;
                    }
                    Tok::Space => gap = true,
                    Tok::Newline => {
                        w += 1;
                        gap = false;
                    }
                }
            }
            w
        };
        let longest_word = |cell: &[Tok]| {
            cell.iter()
                .map(|t| {
                    if let Tok::Word(p) = t {
                        pieces_width(p)
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0)
        };
        let all: Vec<&Vec<Vec<Tok>>> = std::iter::once(&head).chain(rows.iter()).collect();
        let mut widths: Vec<usize> = (0..ncols)
            .map(|c| all.iter().map(|r| natural(&r[c])).max().unwrap_or(0).max(1))
            .collect();
        let mins: Vec<usize> = (0..ncols)
            .map(|c| {
                all.iter()
                    .map(|r| longest_word(&r[c]))
                    .max()
                    .unwrap_or(1)
                    .max(1)
            })
            .collect();

        let avail = self.avail();
        let overhead = 3 * ncols + 1;
        const FLOOR: usize = 3;
        if avail < overhead + ncols * FLOOR {
            // Too narrow for a grid: fall back to one "a | b | c" line per row.
            for row in std::iter::once(&head).chain(rows.iter()) {
                let mut toks = Vec::new();
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 {
                        toks.extend([
                            Tok::Space,
                            Tok::Word(vec![("|".into(), self.dim())]),
                            Tok::Space,
                        ]);
                    }
                    toks.extend(cell.iter().cloned());
                }
                for line in wrap_toks(&toks, avail) {
                    self.emit(line);
                }
            }
            return;
        }
        let budget = avail - overhead;
        // Shrink the widest column first; respect each column's longest word while possible.
        for floor in [None, Some(FLOOR)] {
            while widths.iter().sum::<usize>() > budget {
                let pick = (0..ncols)
                    .filter(|&c| {
                        widths[c]
                            > floor
                                .unwrap_or(mins[c])
                                .max(FLOOR.min(widths[c]))
                                .max(if floor.is_some() { FLOOR } else { mins[c] })
                    })
                    .max_by_key(|&c| widths[c]);
                match pick {
                    Some(c) => widths[c] -= 1,
                    None => break,
                }
            }
        }

        let border = self.theme.border_style(false);
        let rule = |l: &str, m: &str, r: &str| -> Piece {
            let cells: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            (format!("{l}{}{r}", cells.join(m)), border)
        };
        self.emit(vec![rule("┌", "┬", "┐")]);
        let render_row = |b: &mut Self, row: &[Vec<Tok>]| {
            let cells: Vec<Vec<Vec<Piece>>> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| wrap_toks(cell, widths[c]))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for i in 0..height {
                let mut line: Vec<Piece> = vec![("│".into(), border)];
                for (c, lines) in cells.iter().enumerate() {
                    let content = lines.get(i).cloned().unwrap_or_default();
                    let slack = widths[c].saturating_sub(pieces_width(&content));
                    let (left, right) = match t.aligns.get(c) {
                        Some(Alignment::Right) => (slack, 0),
                        Some(Alignment::Center) => (slack / 2, slack - slack / 2),
                        _ => (0, slack),
                    };
                    line.push((" ".repeat(left + 1), Style::default()));
                    line.extend(content);
                    line.push((" ".repeat(right + 1), Style::default()));
                    line.push(("│".into(), border));
                }
                b.emit(line);
            }
        };
        render_row(self, &head);
        self.emit(vec![rule("├", "┼", "┤")]);
        for row in &rows {
            render_row(self, row);
        }
        self.emit(vec![rule("└", "┴", "┘")]);
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        if self.code.is_some() {
            self.render_code();
        }
        while self.out.last().is_some_and(|l| {
            l.spans
                .iter()
                .all(|s| s.content.trim().is_empty() || s.content.trim() == "│")
        }) {
            self.out.pop();
        }
        self.out
    }
}

/// Render `md` into lines no wider than `width`, using `theme` for colors.
pub fn render(md: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = Builder::new(theme, width.max(1));
    for ev in Parser::new_ext(md, opts) {
        b.event(ev);
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::builtin_first()
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn md(src: &str, w: usize) -> Vec<String> {
        text(&render(src, w, &theme()))
    }

    /// The style of the first span containing `needle`.
    fn style_of(lines: &[Line], needle: &str) -> Style {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not rendered"))
            .style
    }

    const SAMPLE: &str = "# Title\n\nSome **bold**, *italic*, ~~gone~~ and `code` with a [link](https://example.com/x).\n\n\
> quoted *text*\n> second line\n\n- one\n- two\n  - nested\n  1. deep\n- [x] done\n- [ ] todo\n\n1. first\n2. second\n\n---\n\n\
```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n| Name | Qty | Notes |\n|:-----|----:|:-----:|\n| apple | 3 | tasty fruit |\n| kiwi | 12 | green |\n\nTail paragraph.";

    #[test]
    fn plain_paragraphs_wrap_and_separate() {
        assert_eq!(
            md("one two three four\n\nsecond", 9),
            ["one two", "three", "four", "", "second"]
        );
        assert_eq!(md("soft\nbreak", 20), ["soft break"]);
        assert_eq!(md("hard  \nbreak", 20), ["hard", "break"]);
        assert!(md("", 20).is_empty() && md("   \n\n  ", 20).is_empty());
    }

    #[test]
    fn inline_styles_apply_to_the_right_words() {
        let lines = render("a **bold** *it* ~~x~~ `c` [lnk](http://u.io)", 80, &theme());
        assert!(
            style_of(&lines, "bold")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_of(&lines, "it")
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            style_of(&lines, "x")
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert_eq!(
            style_of(&lines, "c").bg,
            Some(theme().sel_bg),
            "inline code has its own background"
        );
        assert!(
            style_of(&lines, "lnk")
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(!style_of(&lines, "a").add_modifier.contains(Modifier::BOLD));
        // Punctuation glued to emphasis stays glued: no break between `**bold**` and `,`.
        assert_eq!(md("**bold**, next", 7), ["bold,", "next"]);
    }

    #[test]
    fn links_show_their_url_unless_it_is_the_text() {
        assert_eq!(md("[docs](https://x.io/a)", 80), ["docs (https://x.io/a)"]);
        assert_eq!(md("<https://x.io/a>", 80), ["https://x.io/a"]);
        assert_eq!(md("![a cat](cat.png)", 80), ["[image: a cat]"]);
    }

    #[test]
    fn headings_get_rules_by_level() {
        assert_eq!(
            md("# Big title\ntext", 40),
            ["Big title", "═════════", "", "text"]
        );
        assert_eq!(md("## Mid", 40), ["Mid", "───"]);
        assert_eq!(md("### Small", 40), ["Small"]);
        let lines = render("# H", 20, &theme());
        assert_eq!(style_of(&lines, "H").fg, Some(theme().accent));
    }

    #[test]
    fn lists_nest_number_and_wrap_with_hanging_indents() {
        assert_eq!(md("- a\n- b", 20), ["• a", "• b"]);
        assert_eq!(
            md("1. a\n2. b\n10. c", 20),
            ["1. a", "2. b", "3. c"],
            "numbering is sequential"
        );
        assert_eq!(
            md("3. x\n4. y", 20),
            ["3. x", "4. y"],
            "start number is honoured"
        );
        assert_eq!(md("- a\n  - b\n    - c", 20), ["• a", "  ○ b", "    ■ c"]);
        assert_eq!(
            md("- one two three four", 10),
            ["• one two", "  three", "  four"]
        );
        assert_eq!(
            md("- [x] done\n- [ ] todo", 20),
            ["• [x] done", "• [ ] todo"]
        );
        assert_eq!(
            md("para\n\n- a\n\nafter", 20),
            ["para", "", "• a", "", "after"]
        );
    }

    #[test]
    fn block_quotes_carry_a_bar_through_blank_lines() {
        assert_eq!(
            md("> one\n>\n> two\n\nout", 20),
            ["│ one", "│", "│ two", "", "out"]
        );
        assert_eq!(md("> a\n> > b", 20), ["│ a", "│", "│ │ b"]);
        assert_eq!(md("> - item", 20), ["│ • item"]);
    }

    #[test]
    fn code_blocks_are_boxed_not_reflowed_and_keep_indentation() {
        let out = md("```rust\nfn main() {\n    x();\n}\n```", 24);
        let row = |code: &str| format!("│ {code:<20} │");
        assert_eq!(out[0], "┌─┤ rust ├─────────────┐");
        assert_eq!(out[1], row("fn main() {"));
        assert_eq!(out[2], row("    x();"));
        assert_eq!(out[3], row("}"));
        assert_eq!(out[4], format!("└{}┘", "─".repeat(22)));
        assert_eq!(out.len(), 5);
        // No language: plain frame. Long lines are hard-wrapped inside the frame.
        let out = md("```\nabcdefghijklmnop\n```", 12);
        assert_eq!(
            out,
            [
                "┌──────────┐",
                "│ abcdefgh │",
                "│ ijklmnop │",
                "└──────────┘"
            ]
        );
        // An unterminated fence (a reply still streaming) still renders as a box.
        assert_eq!(md("```\nstill typing", 20).len(), 3);
        // Indented code blocks too.
        assert!(
            md("para\n\n    indented()", 20)
                .iter()
                .any(|l| l.contains("indented()"))
        );
    }

    #[test]
    fn tables_use_linked_box_glyphs_and_alignment() {
        let out = md("| A | B |\n|:--|--:|\n| x | longer |\n| yy | z |", 40);
        assert_eq!(
            out,
            [
                "┌────┬────────┐",
                "│ A  │      B │",
                "├────┼────────┤",
                "│ x  │ longer │",
                "│ yy │      z │",
                "└────┴────────┘"
            ]
        );
        let centered = md("| h |\n|:-:|\n| abcde |\n| x |", 40);
        assert!(centered.contains(&"│   x   │".to_string()), "{centered:#?}");
    }

    #[test]
    fn wide_tables_shrink_and_wrap_cells_to_fit() {
        let src = "| Name | Description |\n|---|---|\n| widget | A very long description that cannot possibly fit on one line of this pane |";
        for w in [24, 30, 40, 60] {
            let out = md(src, w);
            assert!(out.iter().all(|l| width(l) <= w), "width {w}: {out:#?}");
            assert!(
                out.len() > 5,
                "the long cell wrapped onto several rows (w={w})"
            );
            assert!(out[0].starts_with('┌') && out.last().unwrap().starts_with('└'));
        }
        // Too narrow for a grid: degrade to one line per row, still within the width.
        let out = md(src, 10);
        assert!(out.iter().all(|l| width(l) <= 10) && !out.is_empty());
    }

    #[test]
    fn hostile_content_cannot_inject_terminal_control_sequences() {
        let src = "text \x1b[2J &#27;[31m &#x07; [click](http://e.com/\x1b]8;;evil) `c\x1b[0m`\n\n```\x1b[1mlang\ncode \x1b[2J\n```\n\n<script>\x1b[0m</script>\n\n| \x1b[2J | b |\n|--|--|\n| &#27;x | y |";
        for w in [1, 5, 20, 80] {
            for line in text(&render(src, w, &theme())) {
                assert!(!line.chars().any(char::is_control), "width {w}: {line:?}");
            }
        }
        assert!(md("a &amp; b &lt;tag&gt;", 40)[0].contains("a & b <tag>"));
    }

    #[test]
    fn lines_never_exceed_the_width_for_any_width_or_prefix_of_the_document() {
        let t = theme();
        let nested = "> > > > > > > deep quote with plenty of words to wrap around\n\n- a\n  - b\n    - c\n      - d\n        - e long long long text here\n\n日本語のテキストは幅が二倍です。 ".repeat(2);
        for doc in [SAMPLE, nested.as_str()] {
            for w in [1, 2, 3, 4, 7, 12, 25, 40, 80] {
                for line in text(&render(doc, w, &t)) {
                    assert!(width(&line) <= w.max(2), "width {w}: {line:?}"); // a wide glyph needs 2 columns
                }
            }
            // Every prefix of the document (what a streaming reply looks like) renders, in bounds.
            for end in (0..=doc.len()).filter(|&i| doc.is_char_boundary(i)) {
                for line in text(&render(&doc[..end], 30, &t)) {
                    assert!(width(&line) <= 30, "prefix {end}: {line:?}");
                }
            }
        }
    }

    #[test]
    fn a_realistic_reply_renders_every_block_type() {
        let out = md(SAMPLE, 60);
        let joined = out.join("\n");
        assert!(md(SAMPLE, 120).iter().any(|l| {
            l.contains("Some bold, italic, gone and code with a link (https://example.com/x).")
        }));
        for want in [
            "Title",
            "═════",
            "│ quoted text",
            "• one",
            "  ○ nested",
            "  1. deep",
            "• [x] done",
            "1. first",
            "┌─┤ rust ├",
            "│ fn main() {",
            "┌",
            "Tail paragraph.",
        ] {
            assert!(joined.contains(want), "missing {want:?}\n{joined}");
        }
        // Blocks are separated by exactly one blank line, never two in a row.
        assert!(
            !out.windows(2)
                .any(|w| w[0].trim().is_empty() && w[1].trim().is_empty()),
            "{joined}"
        );
        assert!(!out.first().unwrap().trim().is_empty() && !out.last().unwrap().trim().is_empty());
    }
}
