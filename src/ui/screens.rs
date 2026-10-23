//! Drawing for the two screens. All borders go through one `LineGrid`, so panes and
//! table columns that touch are joined with box-drawing junctions. Drawing reads
//! `App`; the only state it touches is scroll clamping and table offsets.
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, HighlightSpacing, Paragraph, Row, Table, TableState};

use super::draw::{fit, hints};
use super::frame::{LineGrid, border_title, meter, scrollbar};
use super::text::{format_tokens, safe_text, wrap};
use super::theme::Theme;
use crate::app::{App, Focus, NotebookView, Screen, Severity};
use crate::llm::provider::ModelRef;
use crate::models::{Message, Source};

const MIN_WIDTH: u16 = 50;
const MIN_HEIGHT: u16 = 12;
const PREFIX_WIDTH: usize = 6;
const SPINNER: [&str; 4] = ["▀", "▐", "▄", "▌"];
const HIGHLIGHT: &str = "► ";
const HIGHLIGHT_W: u16 = 2;
const METER_CELLS: usize = 10;

/// Untrusted text on a single line (table cells, titles).
fn one_line(s: &str) -> String {
    safe_text(s).replace('\n', " ")
}

fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick / 4) as usize % SPINNER.len()]
}

pub fn draw(app: &mut App, f: &mut Frame) {
    let theme = app.theme.clone();
    let area = f.area();
    f.buffer_mut().set_style(area, theme.base());
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!("Terminal too small (need {MIN_WIDTH}x{MIN_HEIGHT})");
        f.render_widget(Paragraph::new(msg).style(theme.base()), area);
        return;
    }
    let [header, body, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(app, f, header, &theme);
    match app.screen {
        Screen::List => draw_list(app, f, body, &theme),
        Screen::Notebook => draw_notebook(app, f, body, &theme),
    }
    draw_status(app, f, status, &theme);
    draw_hints(app, f, keys, &theme);

    if let Some(modal) = app.modal.as_mut() {
        modal.draw(f, area, &theme);
    }
}

fn draw_header(app: &App, f: &mut Frame, area: Rect, theme: &Theme) {
    let left = match (&app.screen, &app.nb) {
        (Screen::Notebook, Some(nb)) => format!(" ≡ rsnotebook » {}", one_line(&nb.name)),
        _ => " ≡ rsnotebook » Notebooks".to_string(),
    };
    let right = format!("{} · ^T theme · F1 help ", one_line(&theme.name));
    let w = area.width as usize;
    let left = fit(&left, w.saturating_sub(right.chars().count() + 1));
    let pad = w.saturating_sub(super::text::width(&left) + right.chars().count());
    f.render_widget(
        Paragraph::new(format!("{left}{}{right}", " ".repeat(pad))).style(theme.bar()),
        area,
    );
}

fn draw_status(app: &App, f: &mut Frame, area: Rect, theme: &Theme) {
    let w = area.width as usize;
    f.render_widget(Paragraph::new("").style(theme.strip()), area);
    if let Some(t) = &app.toast {
        let color = match t.severity {
            Severity::Info => theme.green,
            Severity::Warning => theme.yellow,
            Severity::Error => theme.red,
        };
        let style = Style::default()
            .fg(color)
            .bg(theme.sel_bg)
            .add_modifier(ratatui::style::Modifier::BOLD);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ● ", style),
                Span::styled(fit(&one_line(&t.text), w.saturating_sub(4)), style),
            ])),
            area,
        );
        return;
    }
    let line = match (&app.screen, &app.nb) {
        (Screen::Notebook, Some(nb)) => notebook_status(app, nb, theme),
        _ => {
            let filter = if app.list.filter.is_empty() {
                String::new()
            } else {
                format!(" │ filter: {}", one_line(&app.list.filter))
            };
            Line::styled(
                format!(" {} notebook(s){filter}", app.list.rows.len()),
                theme.strip(),
            )
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

pub fn notebook_status(app: &App, nb: &NotebookView, theme: &Theme) -> Line<'static> {
    let strip = theme.strip();
    let sep = Span::styled(" │ ", strip);
    let enabled: Vec<&Source> = nb.sources.iter().filter(|s| s.enabled).collect();
    let used: usize = enabled.iter().map(|s| s.token_estimate).sum();
    let budget = app.cfg.context_token_budget;
    let model = nb
        .model
        .clone()
        .or_else(|| app.cfg.default_model.clone())
        .map(|m| ModelRef::parse(&m).short())
        .unwrap_or_else(|| "(no model)".into());

    let mut spans = vec![
        Span::styled(
            format!(" {}", one_line(&model)),
            Style::default()
                .fg(theme.accent2)
                .bg(theme.sel_bg)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled("ctx ", strip),
    ];
    spans.extend(meter(used, budget, METER_CELLS, theme, strip));
    spans.push(Span::styled(
        format!(" {}/{}", format_tokens(used), format_tokens(budget)),
        theme.fg_on_strip(),
    ));
    spans.push(sep);
    spans.push(Span::styled(
        format!("{} source(s)", enabled.len()),
        theme.fg_on_strip(),
    ));
    let busy = Style::default().fg(theme.yellow).bg(theme.sel_bg);
    if nb.generating {
        spans.push(Span::styled(
            format!(" │ generating {}", spinner(app.tick)),
            busy,
        ));
    }
    if nb.importing > 0 {
        spans.push(Span::styled(
            format!(" │ importing {}", spinner(app.tick)),
            busy,
        ));
    }
    Line::from(spans)
}

fn draw_hints(app: &App, f: &mut Frame, area: Rect, theme: &Theme) {
    let w = area.width as usize;
    f.render_widget(Paragraph::new("").style(theme.strip()), area);
    let line = match (&app.screen, &app.nb) {
        (Screen::Notebook, Some(nb)) if nb.focus == Focus::Sources => hints(
            &[
                ("Tab", "chat"),
                ("a", "add"),
                ("Space", "toggle"),
                ("x", "remove"),
                ("r", "reimport"),
                ("Enter", "preview"),
                ("^O", "model"),
                ("^L", "clear"),
                ("^T", "theme"),
                ("Esc", "back"),
            ],
            w,
            theme,
        ),
        (Screen::Notebook, Some(nb)) => hints(
            &[
                ("Enter", if nb.generating { "wait" } else { "send" }),
                ("^J", "newline"),
                ("Tab", "sources"),
                ("PgUp/Dn", "scroll"),
                ("^O", "model"),
                ("^L", "clear"),
                ("^T", "theme"),
                ("F2", "markdown"),
                ("Esc", if nb.generating { "cancel" } else { "back" }),
            ],
            w,
            theme,
        ),
        _ => hints(
            &[
                ("n", "new"),
                ("e", "edit"),
                ("d", "delete"),
                ("Enter", "open"),
                ("/", "filter"),
                ("m", "model"),
                ("t", "theme"),
                ("^K", "providers"),
                ("F1", "help"),
                ("q", "quit"),
            ],
            w,
            theme,
        ),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Separator x positions for columns of `widths` separated by `gap` cells, starting at `x0`.
fn separators(x0: u16, widths: &[u16], gap: u16) -> Vec<u16> {
    let mut xs = Vec::new();
    let mut x = x0;
    for w in &widths[..widths.len().saturating_sub(1)] {
        x += w;
        xs.push(x + gap / 2);
        x += gap;
    }
    xs
}

fn header_row<'a>(cells: &[&'a str], theme: &Theme) -> Row<'a> {
    Row::new(cells.iter().map(|c| Cell::from(*c)))
        .style(theme.accent_style())
        .bottom_margin(1)
}

fn make_table<'a>(
    rows: Vec<Row<'a>>,
    header: Row<'a>,
    widths: &[u16],
    gap: u16,
    theme: &Theme,
) -> Table<'a> {
    Table::new(
        rows,
        widths
            .iter()
            .map(|w| Constraint::Length(*w))
            .collect::<Vec<_>>(),
    )
    .header(header)
    .column_spacing(gap)
    .highlight_symbol(HIGHLIGHT)
    .highlight_spacing(HighlightSpacing::Always)
    .row_highlight_style(theme.selected())
    .style(theme.base())
}

/// Scrollbar over the right border for a table whose rows start two lines below `inner.y`.
fn table_scrollbar(
    f: &mut Frame,
    border_x: u16,
    inner: Rect,
    total: usize,
    state: &TableState,
    theme: &Theme,
) {
    let view = inner.height.saturating_sub(2);
    scrollbar(
        f.buffer_mut(),
        border_x,
        inner.y + 2,
        view,
        total,
        view as usize,
        state.offset(),
        theme,
    );
}

fn draw_list(app: &mut App, f: &mut Frame, body: Rect, theme: &Theme) {
    let inner = Rect::new(body.x + 1, body.y + 1, body.width - 2, body.height - 2);
    let mut grid = LineGrid::new(body);
    grid.rect(body, true);
    let title = if app.list.filter.is_empty() {
        "Notebooks".to_string()
    } else {
        format!(
            "Notebooks / filter: {}",
            fit(&one_line(&app.list.filter), 30)
        )
    };

    if app.list.rows.is_empty() {
        let msg = if app.list.filter.is_empty() {
            "No notebooks yet. Press 'n' to create one.".to_string()
        } else {
            format!(
                "No notebooks match '{}'. Press Esc to clear the filter.",
                fit(&one_line(&app.list.filter), 30)
            )
        };
        f.render_widget(
            Paragraph::new(msg).style(theme.dim_style()),
            Rect::new(inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), 1),
        );
        grid.render(f.buffer_mut(), theme);
        border_title(f.buffer_mut(), body, &title, true, theme);
        return;
    }

    const GAP: u16 = 3;
    let total = inner.width.saturating_sub(HIGHLIGHT_W);
    let show_updated = total >= 74;
    let (w_src, w_upd) = (8, if show_updated { 16 } else { 0 });
    let cols = if show_updated { 4 } else { 3 };
    let name_w = (total * 28 / 100).max(12);
    let desc_w = total
        .saturating_sub(name_w + w_src + w_upd + GAP * (cols - 1))
        .max(4);
    let widths: Vec<u16> = if show_updated {
        vec![name_w, desc_w, w_src, w_upd]
    } else {
        vec![name_w, desc_w, w_src]
    };

    let rows: Vec<Row> = app
        .list
        .rows
        .iter()
        .map(|(nb, count)| {
            let mut cells = vec![
                Cell::from(one_line(&nb.name)),
                Cell::from(one_line(&nb.description)),
                Cell::from(count.to_string()),
            ];
            if show_updated {
                cells.push(Cell::from(
                    nb.updated_at.chars().take(16).collect::<String>(),
                ));
            }
            Row::new(cells)
        })
        .collect();
    let header = header_row(
        if show_updated {
            &["Name", "Description", "Sources", "Updated"]
        } else {
            &["Name", "Description", "Sources"]
        },
        theme,
    );
    f.render_stateful_widget(
        make_table(rows, header, &widths, GAP, theme),
        inner,
        &mut app.list.table,
    );

    for x in separators(inner.x + HIGHLIGHT_W, &widths, GAP) {
        grid.vline(x, body.y, body.bottom() - 1, true);
    }
    grid.hline(body.x, body.right() - 1, inner.y + 1, true);
    grid.render(f.buffer_mut(), theme);
    border_title(f.buffer_mut(), body, &title, true, theme);
    table_scrollbar(
        f,
        body.right() - 1,
        inner,
        app.list.rows.len(),
        &app.list.table,
        theme,
    );
}

fn draw_notebook(app: &mut App, f: &mut Frame, body: Rect, theme: &Theme) {
    let modal_open = app.modal.is_some();
    let left_w = (body.width * 36 / 100)
        .max(28)
        .min(body.width.saturating_sub(24));
    let input_h = 5.min(body.height.saturating_sub(4));
    let chat_h = body.height - input_h + 1; // the two rects share one border row
    let left = Rect::new(body.x, body.y, left_w, body.height);
    let chat = Rect::new(body.x + left_w - 1, body.y, body.width - left_w + 1, chat_h);
    let input = Rect::new(chat.x, body.y + chat_h - 1, chat.width, input_h);
    let tick = app.tick;
    let markdown = app.markdown;
    let Some(nb) = app.nb.as_mut() else { return };

    let active = nb.focus == Focus::Chat && !nb.generating;
    let mut grid = LineGrid::new(body);
    grid.rect(left, nb.focus == Focus::Sources);
    grid.rect(chat, false);
    grid.rect(input, active);

    // Sources table
    let s_inner = Rect::new(left.x + 1, left.y + 1, left.width - 2, left.height - 2);
    if nb.sources.is_empty() {
        let msg = "No sources yet. Press Tab to focus sources, then 'a' to add files.";
        f.render_widget(
            Paragraph::new(msg)
                .style(theme.dim_style())
                .wrap(ratatui::widgets::Wrap { trim: true }),
            Rect::new(
                s_inner.x + 1,
                s_inner.y + 1,
                s_inner.width.saturating_sub(2),
                s_inner.height.saturating_sub(1),
            ),
        );
    } else {
        const GAP: u16 = 1;
        let fixed = [2, 0, 5, 5];
        let name_w = s_inner
            .width
            .saturating_sub(HIGHLIGHT_W + fixed.iter().sum::<u16>() + GAP * 3)
            .max(4);
        let widths = [fixed[0], name_w, fixed[2], fixed[3]];
        let rows: Vec<Row> = nb
            .sources
            .iter()
            .map(|s| {
                let style = if s.status == "error" {
                    theme.error_style()
                } else if !s.enabled {
                    theme.dim_style()
                } else {
                    theme.base()
                };
                let state = if s.status == "ok" { "ok" } else { "err" };
                Row::new([
                    Cell::from(if s.enabled { "●" } else { "○" }),
                    Cell::from(one_line(&s.filename)),
                    Cell::from(state),
                    Cell::from(format_tokens(s.token_estimate)),
                ])
                .style(style)
            })
            .collect();
        let header = header_row(&["", "File", "State", "Tok"], theme);
        f.render_stateful_widget(
            make_table(rows, header, &widths, GAP, theme),
            s_inner,
            &mut nb.src_table,
        );
        for x in separators(s_inner.x + HIGHLIGHT_W, &widths, GAP) {
            grid.vline(x, left.y, left.bottom() - 1, nb.focus == Focus::Sources);
        }
        grid.hline(
            left.x,
            left.right() - 1,
            s_inner.y + 1,
            nb.focus == Focus::Sources,
        );
    }

    // Chat log
    let c_inner = Rect::new(
        chat.x + 2,
        chat.y + 1,
        chat.width.saturating_sub(4),
        chat.height.saturating_sub(2),
    );
    let width = c_inner.width as usize;
    let key: ChatKey = (
        nb.messages.len(),
        nb.messages.first().map_or(0, |m| m.id),
        nb.messages.last().map_or(0, |m| m.id),
        width,
        theme.id.clone(),
        markdown,
    );
    if nb.chat_cache.as_ref().is_none_or(|c| c.key != key) {
        nb.chat_cache = Some(ChatCache {
            key,
            lines: history_lines(&nb.messages, width, theme, markdown),
        });
    }
    let live = if nb.generating {
        live_lines(&nb.live, tick, width, theme, markdown)
    } else {
        Vec::new()
    };
    let history = nb.chat_cache.as_ref().map_or(&[][..], |c| &c.lines[..]);
    let total = history.len() + live.len();
    let view_h = c_inner.height as usize;
    let max_scroll = total.saturating_sub(view_h);
    nb.scroll = nb.scroll.min(max_scroll);
    let start = max_scroll - nb.scroll;
    if nb.messages.is_empty() && !nb.generating {
        f.render_widget(
            Paragraph::new("Ask a question about your enabled sources below.")
                .style(theme.dim_style()),
            Rect::new(c_inner.x, c_inner.y + 1, c_inner.width, 1),
        );
    } else {
        let shown: Vec<Line> = history
            .iter()
            .chain(live.iter())
            .skip(start)
            .take(view_h)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(shown).style(theme.base()), c_inner);
    }

    // Message input
    let i_inner = Rect::new(
        input.x + 2,
        input.y + 1,
        input.width.saturating_sub(4),
        input.height.saturating_sub(2),
    );
    if nb.generating {
        f.render_widget(
            Paragraph::new("Generating... press Esc to cancel.").style(theme.dim_style()),
            i_inner,
        );
    } else if nb.input.is_empty() {
        f.render_widget(
            Paragraph::new("Type a question. Enter sends, Ctrl+J adds a newline.")
                .style(theme.dim_style()),
            i_inner,
        );
        if active && !modal_open {
            f.set_cursor_position((i_inner.x, i_inner.y));
        }
    } else {
        let (rows, (col, row)) = nb
            .input
            .view(i_inner.width as usize, i_inner.height as usize);
        f.render_widget(Paragraph::new(rows.join("\n")).style(theme.base()), i_inner);
        if active && !modal_open {
            f.set_cursor_position((i_inner.x + col as u16, i_inner.y + row as u16));
        }
    }

    // Lines and captions go on last so they sit over any text that reached an edge.
    grid.render(f.buffer_mut(), theme);
    let buf = f.buffer_mut();
    border_title(
        buf,
        left,
        &format!("Sources ({})", nb.sources.len()),
        nb.focus == Focus::Sources,
        theme,
    );
    border_title(
        buf,
        chat,
        if nb.scroll > 0 {
            "Chat · scrolled, PgDn for newest"
        } else {
            "Chat"
        },
        false,
        theme,
    );
    border_title(
        buf,
        input,
        if nb.generating {
            "Message · locked"
        } else {
            "Message"
        },
        active,
        theme,
    );
    scrollbar(
        buf,
        chat.right() - 1,
        chat.y + 1,
        chat.height - 2,
        total,
        view_h,
        start,
        theme,
    );
    let s_view = s_inner.height.saturating_sub(2);
    scrollbar(
        buf,
        left.right() - 1,
        s_inner.y + 2,
        s_view,
        nb.sources.len(),
        s_view as usize,
        nb.src_table.offset(),
        theme,
    );
}

/// What the cached history lines were rendered for; any change re-renders them.
pub type ChatKey = (usize, i64, i64, usize, String, bool);

/// Rendered history, kept between frames: only the streaming reply changes while it arrives.
pub struct ChatCache {
    key: ChatKey,
    lines: Vec<Line<'static>>,
}

fn line_width(l: &Line) -> usize {
    l.spans.iter().map(|s| super::text::width(&s.content)).sum()
}

fn plain_body(text: &str, body_w: usize, style: Style) -> Vec<Line<'static>> {
    wrap(&safe_text(text), body_w)
        .into_iter()
        .map(|l| Line::styled(l, style))
        .collect()
}

/// One message: a colored lead on the first line, a hanging indent after, then a blank line.
fn block(
    prefix: &str,
    color: ratatui::style::Color,
    body: Vec<Line<'static>>,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    let body = if body.is_empty() {
        vec![Line::raw("")]
    } else {
        body
    };
    for (i, line) in body.into_iter().enumerate() {
        let lead = if i == 0 {
            Span::styled(prefix.to_string(), theme.bold_color(color))
        } else {
            Span::styled(" ".repeat(PREFIX_WIDTH), theme.base())
        };
        let mut spans = vec![lead];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    out.push(Line::raw(""));
}

/// The saved conversation as styled, pre-wrapped lines (so scrolling is exact). Assistant
/// replies are rendered as Markdown when `markdown` is on; your own messages and errors
/// are always shown as typed.
pub fn history_lines(
    messages: &[Message],
    width: usize,
    theme: &Theme,
    markdown: bool,
) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(PREFIX_WIDTH).max(1);
    let mut out = Vec::new();
    for m in messages {
        match m.role.as_str() {
            "user" => block(
                "You ► ",
                theme.cyan,
                plain_body(&m.content, body_w, theme.base()),
                theme,
                &mut out,
            ),
            "error" => block(
                "ERR ! ",
                theme.red,
                plain_body(&m.content, body_w, theme.error_style()),
                theme,
                &mut out,
            ),
            _ => {
                let body = if markdown {
                    super::markdown::render(&m.content, body_w, theme)
                } else {
                    plain_body(&m.content, body_w, theme.base())
                };
                block("AI  ◄ ", theme.green, body, theme, &mut out);
            }
        }
    }
    out
}

/// The reply that is still streaming in, with a spinner at its end.
pub fn live_lines(
    live: &str,
    tick: u64,
    width: usize,
    theme: &Theme,
    markdown: bool,
) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(PREFIX_WIDTH).max(1);
    let spin = spinner(tick);
    let body = if live.is_empty() {
        plain_body(&format!("thinking {spin}"), body_w, theme.base())
    } else if markdown {
        let mut b = super::markdown::render(live, body_w, theme);
        let spin_span = Span::styled(spin, theme.accent_style());
        match b.last_mut() {
            Some(last) if line_width(last) < body_w => last.spans.push(spin_span),
            _ => b.push(Line::from(spin_span)),
        }
        b
    } else {
        plain_body(&format!("{live}{spin}"), body_w, theme.base())
    };
    let mut out = Vec::new();
    block("AI  ◄ ", theme.green, body, theme, &mut out);
    out
}

/// History plus the streaming reply, as one list.
pub fn chat_lines(
    messages: &[Message],
    live: &str,
    generating: bool,
    tick: u64,
    width: usize,
    theme: &Theme,
    markdown: bool,
) -> Vec<Line<'static>> {
    let mut out = history_lines(messages, width, theme, markdown);
    if generating {
        out.extend(live_lines(live, tick, width, theme, markdown));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line]) -> Vec<String> {
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

    #[test]
    fn chat_lines_wrap_with_hanging_prefix_and_strip_escapes() {
        let t = Theme::builtin_first();
        let msgs = vec![
            Message::new(1, "user", "hello there \x1b[2Jfriend of mine"),
            Message::new(1, "assistant", "ok"),
            Message::new(1, "error", "bad"),
        ];
        let lines = text_of(&chat_lines(&msgs, "", false, 0, 16, &t, false));
        assert_eq!(
            lines[..4],
            [
                "You ► hello",
                "      there",
                "      [2Jfriend",
                "      of mine"
            ]
        );
        assert!(lines.iter().any(|l| l == "AI  ◄ ok") && lines.iter().any(|l| l == "ERR ! bad"));
        assert!(lines.iter().all(|l| !l.contains('\x1b')));
    }

    #[test]
    fn live_text_shows_a_spinner_and_thinking_placeholder() {
        let t = Theme::builtin_first();
        assert_eq!(
            text_of(&chat_lines(&[], "", true, 0, 40, &t, false))[0],
            "AI  ◄ thinking ▀"
        );
        assert_eq!(
            text_of(&chat_lines(&[], "partial", true, 4, 40, &t, false))[0],
            "AI  ◄ partial▐"
        );
    }

    #[test]
    fn column_separators_sit_in_the_middle_of_each_gap() {
        assert_eq!(separators(10, &[5, 7, 3], 3), vec![16, 26]);
        assert_eq!(separators(0, &[4, 4], 1), vec![4]);
        assert!(separators(0, &[4], 3).is_empty());
    }
}
