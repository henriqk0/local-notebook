//! Modal dialogs. Each dialog owns its widgets; the app maps `Purpose` + the
//! submitted value to an action.
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::draw::{centered, fit};
use super::frame::double_box;
use super::input::TextBuf;
use super::text::{safe_text, wrap};
use super::theme::Theme;
use crate::llm::models_cache::{ModelCache, ModelInfo};
use crate::llm::provider::ProviderConfig;

/// What the dialog was opened for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    NewNotebook,
    EditNotebook(i64),
    DeleteNotebook(i64),
    Filter,
    /// The providers list (Ctrl+K).
    Providers,
    /// Entering the API key for the provider with this id.
    ProviderKey(String),
    AddPath,
    RemoveSource(i64),
    ClearChat,
    /// Pick a model for the open notebook.
    NotebookModel,
    /// Pick the global default model.
    DefaultModel,
    Info,
    Theme,
}

#[derive(Debug)]
pub enum Submitted {
    Form {
        name: String,
        description: String,
    },
    Confirmed,
    Text(String),
    Model {
        id: String,
        set_default: bool,
    },
    Theme(String),
    /// A provider was chosen in the providers list.
    Provider(String),
    /// Remove the saved API key of this provider.
    ClearKey(String),
}

#[derive(Debug)]
pub enum Outcome {
    Pending,
    Cancel,
    Submit(Submitted),
}

pub struct Modal {
    pub purpose: Purpose,
    kind: Kind,
}

enum Kind {
    Form(Form),
    Confirm(Confirm),
    Input(InputDialog),
    Picker(Picker),
    Text(TextView),
    ThemePicker(ThemePicker),
    Providers(ProviderList),
}

struct Form {
    title: String,
    name: TextBuf,
    description: TextBuf,
    focus: usize,
    error: Option<&'static str>,
}

struct Confirm {
    message: String,
    yes: bool,
}

struct InputDialog {
    title: String,
    message: String,
    input: TextBuf,
}

struct Picker {
    models: Vec<ModelInfo>,
    current: Option<String>,
    query: TextBuf,
    filtered: Vec<usize>,
    cursor: usize,
    set_default_on_enter: bool,
    page: usize,
}

struct ProviderRow {
    id: String,
    name: String,
    kind: &'static str,
    status: String,
    ready: bool,
}

struct ProviderList {
    rows: Vec<ProviderRow>,
    cursor: usize,
}

struct ThemePicker {
    themes: Vec<Theme>,
    /// The theme in effect before the dialog opened.
    current: String,
    cursor: usize,
    page: usize,
}

struct TextView {
    title: String,
    text: String,
    scroll: usize,
    page: usize,
}

fn is_enter(key: &KeyEvent) -> bool {
    key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::ALT)
}

impl Modal {
    pub fn form(purpose: Purpose, title: &str, name: &str, description: &str) -> Self {
        let kind = Kind::Form(Form {
            title: title.into(),
            name: TextBuf::new(name),
            description: TextBuf::new(description),
            focus: 0,
            error: None,
        });
        Self { purpose, kind }
    }

    pub fn confirm(purpose: Purpose, message: &str) -> Self {
        Self {
            purpose,
            kind: Kind::Confirm(Confirm {
                message: safe_text(message),
                yes: false,
            }),
        }
    }

    pub fn input(
        purpose: Purpose,
        title: &str,
        message: &str,
        masked: bool,
        initial: &str,
    ) -> Self {
        let mut input = if masked {
            TextBuf::masked()
        } else {
            TextBuf::default()
        };
        input.insert_str(initial);
        Self {
            purpose,
            kind: Kind::Input(InputDialog {
                title: title.into(),
                message: message.into(),
                input,
            }),
        }
    }

    /// `set_default_on_enter`: the global default-model picker always saves as default.
    pub fn picker(
        purpose: Purpose,
        mut models: Vec<ModelInfo>,
        current: Option<String>,
        set_default_on_enter: bool,
    ) -> Self {
        models.sort_by_key(|m| (m.provider.to_lowercase(), m.id.to_lowercase()));
        let filtered: Vec<usize> = (0..models.len()).collect();
        let cursor = current
            .as_ref()
            .and_then(|c| models.iter().position(|m| &m.reference() == c))
            .unwrap_or(0);
        let kind = Kind::Picker(Picker {
            models,
            current,
            query: TextBuf::default(),
            filtered,
            cursor,
            set_default_on_enter,
            page: 10,
        });
        Self { purpose, kind }
    }

    pub fn text(purpose: Purpose, title: &str, text: &str) -> Self {
        Self {
            purpose,
            kind: Kind::Text(TextView {
                title: safe_text(title),
                text: safe_text(text),
                scroll: 0,
                page: 10,
            }),
        }
    }

    /// The providers list: Enter picks one to set up, `d` removes its saved key.
    pub fn providers(providers: &[ProviderConfig]) -> Self {
        let rows = providers
            .iter()
            .map(|p| ProviderRow {
                id: p.id.clone(),
                name: p.name.clone(),
                kind: p.kind.as_str(),
                status: p.status(),
                ready: p.is_ready(),
            })
            .collect();
        Self {
            purpose: Purpose::Providers,
            kind: Kind::Providers(ProviderList { rows, cursor: 0 }),
        }
    }

    pub fn theme_picker(themes: Vec<Theme>, current: &str) -> Self {
        let cursor = themes.iter().position(|t| t.id == current).unwrap_or(0);
        Self {
            purpose: Purpose::Theme,
            kind: Kind::ThemePicker(ThemePicker {
                themes,
                current: current.to_string(),
                cursor,
                page: 8,
            }),
        }
    }

    /// The theme under the cursor, for live preview while the picker is open.
    pub fn highlighted_theme(&self) -> Option<&Theme> {
        match &self.kind {
            Kind::ThemePicker(p) => p.themes.get(p.cursor),
            _ => None,
        }
    }

    pub fn paste(&mut self, s: &str) {
        match &mut self.kind {
            Kind::Form(f) => {
                if f.focus == 0 {
                    f.name.insert_str(s)
                } else {
                    f.description.insert_str(s)
                }
            }
            Kind::Input(i) => i.input.insert_str(s),
            Kind::Picker(p) => {
                p.query.insert_str(s);
                p.refilter();
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match &mut self.kind {
            Kind::Form(f) => f.handle_key(key),
            Kind::Confirm(c) => c.handle_key(key),
            Kind::Input(i) => {
                if key.code == KeyCode::Esc {
                    Outcome::Cancel
                } else if is_enter(&key) {
                    Outcome::Submit(Submitted::Text(i.input.text()))
                } else {
                    i.input.handle_key(key);
                    Outcome::Pending
                }
            }
            Kind::Picker(p) => p.handle_key(key),
            Kind::Text(t) => t.handle_key(key),
            Kind::ThemePicker(p) => p.handle_key(key),
            Kind::Providers(p) => p.handle_key(key),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &mut self.kind {
            Kind::Form(f) => f.draw(frame, area, theme),
            Kind::Confirm(c) => c.draw(frame, area, theme),
            Kind::Input(i) => i.draw(frame, area, theme),
            Kind::Picker(p) => p.draw(frame, area, theme),
            Kind::Text(t) => t.draw(frame, area, theme),
            Kind::ThemePicker(p) => p.draw(frame, area, theme),
            Kind::Providers(p) => p.draw(frame, area, theme),
        }
    }
}

/// Clear and frame a dialog, returning its inner area.
fn open(frame: &mut Frame, area: Rect, w: u16, h: u16, title: &str, theme: &Theme) -> Rect {
    let rect = centered(area, w, h);
    double_box(frame.buffer_mut(), rect, title, theme)
}

/// Draw a labelled single-line field and place the terminal cursor if focused.
fn field(frame: &mut Frame, area: Rect, label: &str, buf: &TextBuf, focused: bool, theme: &Theme) {
    let label_w = if label.is_empty() {
        0
    } else {
        label.len() as u16 + 2
    };
    if label_w > 0 {
        frame.render_widget(
            Paragraph::new(format!("{label}: ")).style(theme.base()),
            Rect::new(area.x, area.y, label_w.min(area.width), 1),
        );
    }
    let w = area.width.saturating_sub(label_w) as usize;
    let (rows, (col, _)) = buf.view(w, 1);
    let style = if focused {
        Style::default().fg(theme.fg).bg(theme.sel_bg)
    } else {
        theme.dim_style()
    };
    let text = format!("{:<width$}", rows[0], width = w);
    frame.render_widget(
        Paragraph::new(text).style(style),
        Rect::new(area.x + label_w, area.y, w as u16, 1),
    );
    if focused {
        frame.set_cursor_position((area.x + label_w + col as u16, area.y));
    }
}

impl Form {
    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                self.focus = 1 - self.focus
            }
            KeyCode::Enter if self.focus == 0 => self.focus = 1,
            KeyCode::Enter => return self.save(),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.save();
            }
            _ => {
                let buf = if self.focus == 0 {
                    &mut self.name
                } else {
                    &mut self.description
                };
                buf.handle_key(key);
                self.error = None;
            }
        }
        Outcome::Pending
    }

    fn save(&mut self) -> Outcome {
        if self.name.text().trim().is_empty() {
            self.error = Some("Name is required.");
            self.focus = 0;
            return Outcome::Pending;
        }
        Outcome::Submit(Submitted::Form {
            name: self.name.text(),
            description: self.description.text(),
        })
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let inner = open(frame, area, 64, 9, &self.title, theme);
        let w = inner.width.saturating_sub(2);
        field(
            frame,
            Rect::new(inner.x + 1, inner.y + 1, w, 1),
            "Name       ",
            &self.name,
            self.focus == 0,
            theme,
        );
        field(
            frame,
            Rect::new(inner.x + 1, inner.y + 3, w, 1),
            "Description",
            &self.description,
            self.focus == 1,
            theme,
        );
        if let Some(e) = self.error {
            frame.render_widget(
                Paragraph::new(e).style(theme.error_style()),
                Rect::new(inner.x + 1, inner.y + 4, w, 1),
            );
        }
        let hint = "Tab switch   Enter next/save   Ctrl+S save   Esc cancel";
        let y = inner.y + inner.height.saturating_sub(1);
        frame.render_widget(
            Paragraph::new(fit(hint, w as usize)).style(theme.dim_style()),
            Rect::new(inner.x + 1, y, w, 1),
        );
    }
}

impl Confirm {
    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Char('y' | 'Y') => Outcome::Submit(Submitted::Confirmed),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Outcome::Cancel,
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h' | 'l') => {
                self.yes = !self.yes;
                Outcome::Pending
            }
            KeyCode::Enter if self.yes => Outcome::Submit(Submitted::Confirmed),
            KeyCode::Enter => Outcome::Cancel,
            _ => Outcome::Pending,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let w = (self.message.chars().count() as u16 + 6).clamp(36, 70);
        let lines = wrap(&self.message, w.saturating_sub(4) as usize);
        let inner = open(frame, area, w, lines.len() as u16 + 5, "Confirm", theme);
        let body = Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            lines.len() as u16,
        );
        frame.render_widget(Paragraph::new(lines.join("\n")).style(theme.base()), body);
        let (yes, no) = if self.yes {
            (theme.selected(), theme.base())
        } else {
            (theme.base(), theme.selected())
        };
        let buttons = Line::from(vec![
            Span::styled(" Yes (y) ", yes),
            Span::styled("   ", theme.base()),
            Span::styled(" No (n) ", no),
        ]);
        frame.render_widget(
            Paragraph::new(buttons),
            Rect::new(
                inner.x + 2,
                inner.y + inner.height.saturating_sub(2),
                inner.width.saturating_sub(2),
                1,
            ),
        );
    }
}

impl InputDialog {
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let w = 70.min(area.width);
        let lines = wrap(&self.message, w.saturating_sub(4) as usize);
        let inner = open(frame, area, w, lines.len() as u16 + 7, &self.title, theme);
        let fw = inner.width.saturating_sub(2);
        frame.render_widget(
            Paragraph::new(lines.join("\n")).style(theme.base()),
            Rect::new(inner.x + 1, inner.y + 1, fw, lines.len() as u16),
        );
        let y = inner.y + 2 + lines.len() as u16;
        field(
            frame,
            Rect::new(inner.x + 1, y, fw, 1),
            "",
            &self.input,
            true,
            theme,
        );
        frame.render_widget(
            Paragraph::new("Enter confirm   Esc cancel").style(theme.dim_style()),
            Rect::new(inner.x + 1, y + 2, fw, 1),
        );
    }
}

impl Picker {
    fn refilter(&mut self) {
        let wanted = self.filtered.get(self.cursor).copied();
        let q = self.query.text();
        self.filtered = (0..self.models.len())
            .filter(|&i| ModelCache::matches(&self.models[i], &q))
            .collect();
        self.cursor = wanted
            .and_then(|w| self.filtered.iter().position(|&i| i == w))
            .unwrap_or(0);
    }

    fn choose(&self, set_default: bool) -> Outcome {
        match self.filtered.get(self.cursor) {
            Some(&i) => Outcome::Submit(Submitted::Model {
                id: self.models[i].reference(),
                set_default: set_default || self.set_default_on_enter,
            }),
            None => Outcome::Pending,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.filtered.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => return self.choose(false),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.choose(true);
            }
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down => self.cursor = (self.cursor + 1).min(last),
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(self.page),
            KeyCode::PageDown => self.cursor = (self.cursor + self.page).min(last),
            _ => {
                if self.query.handle_key(key) {
                    self.refilter();
                }
            }
        }
        Outcome::Pending
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let inner = open(
            frame,
            area,
            area.width.min(104),
            area.height.min(24),
            "Select a model",
            theme,
        );
        let inner = Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            inner.height,
        );
        field(
            frame,
            Rect::new(inner.x, inner.y, inner.width, 1),
            "Search",
            &self.query,
            true,
            theme,
        );

        let list_h = inner.height.saturating_sub(4) as usize;
        self.page = list_h.max(1);
        const PROV_W: usize = 12;
        let id_w = (inner.width as usize)
            .saturating_sub(2 + PROV_W + 1 + 9 + 12 + 12 + 3)
            .max(10);
        let header = format!(
            "  {:<PROV_W$} {:<id_w$} {:>9} {:>12} {:>12}",
            "Provider", "Model", "Context", "Prompt $/1M", "Compl $/1M"
        );
        frame.render_widget(
            Paragraph::new(fit(&header, inner.width as usize)).style(theme.accent_style()),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );

        let top = (self.cursor + 1).saturating_sub(list_h.max(1));
        let mut lines = Vec::new();
        for (row, &i) in self.filtered.iter().enumerate().skip(top).take(list_h) {
            let m = &self.models[i];
            let mark = if self.current.as_deref() == Some(m.reference().as_str()) {
                "● "
            } else {
                "  "
            };
            let label = format!(
                "{}{}",
                safe_text(&m.id),
                if m.is_free { " (free)" } else { "" }
            );
            let price = |p: f64| {
                if m.priced {
                    format!("{:.2}", p * 1_000_000.0)
                } else {
                    "-".to_string()
                }
            };
            let context = if m.context_length > 0 {
                m.context_length.to_string()
            } else {
                "-".to_string()
            };
            let text = format!(
                "{mark}{:<PROV_W$} {:<id_w$} {:>9} {:>12} {:>12}",
                fit(&safe_text(&m.provider), PROV_W),
                fit(&label, id_w),
                context,
                price(m.prompt_price),
                price(m.completion_price)
            );
            let style = if row == self.cursor {
                theme.selected()
            } else {
                theme.base()
            };
            lines.push(Line::styled(fit(&text, inner.width as usize), style));
        }
        if lines.is_empty() {
            lines.push(Line::styled("No models match.", theme.dim_style()));
        }
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y + 3, inner.width, list_h as u16),
        );
        super::frame::scrollbar(
            frame.buffer_mut(),
            inner.right(),
            inner.y + 3,
            list_h as u16,
            self.filtered.len(),
            list_h,
            top,
            theme,
        );
        let hint = if self.set_default_on_enter {
            "Enter set default   Esc cancel"
        } else {
            "Enter use   Ctrl+D use & set default   Esc cancel"
        };
        frame.render_widget(
            Paragraph::new(hint).style(theme.dim_style()),
            Rect::new(
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width,
                1,
            ),
        );
    }
}

impl TextView {
    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => return Outcome::Cancel,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += self.page,
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = usize::MAX / 2,
            _ => {}
        }
        Outcome::Pending
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let w = area.width.saturating_sub(6).clamp(20, 100);
        let h = area.height.saturating_sub(4).max(6);
        let inner = open(
            frame,
            area,
            w,
            h,
            &fit(&self.title, w.saturating_sub(8) as usize),
            theme,
        );
        let text_area = Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(1),
        );
        let lines = wrap(&self.text, text_area.width as usize);
        self.page = text_area.height.max(1) as usize;
        self.scroll = self.scroll.min(lines.len().saturating_sub(self.page));
        let shown: Vec<Line> = lines
            .iter()
            .skip(self.scroll)
            .take(self.page)
            .map(|l| Line::styled(l.clone(), theme.base()))
            .collect();
        frame.render_widget(Paragraph::new(shown), text_area);
        super::frame::scrollbar(
            frame.buffer_mut(),
            inner.right(),
            inner.y,
            text_area.height,
            lines.len(),
            self.page,
            self.scroll,
            theme,
        );
        let more = if lines.len() > self.page {
            format!(
                "lines {}-{} of {}   ",
                self.scroll + 1,
                (self.scroll + self.page).min(lines.len()),
                lines.len()
            )
        } else {
            String::new()
        };
        let hint = format!("{more}Up/Down PgUp/PgDn scroll   Esc close");
        let y = inner.y + inner.height.saturating_sub(1);
        frame.render_widget(
            Paragraph::new(fit(&hint, inner.width as usize)).style(theme.dim_style()),
            Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1),
        );
    }
}

impl ProviderList {
    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancel,
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.cursor = (self.cursor + 1).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = last,
            KeyCode::Enter => {
                if let Some(r) = self.rows.get(self.cursor) {
                    return Outcome::Submit(Submitted::Provider(r.id.clone()));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(r) = self.rows.get(self.cursor) {
                    return Outcome::Submit(Submitted::ClearKey(r.id.clone()));
                }
            }
            _ => {}
        }
        Outcome::Pending
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let h = (self.rows.len() as u16 + 7).min(area.height);
        let inner = open(frame, area, area.width.min(88), h, "Providers", theme);
        let inner = Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(1),
        );
        let name_w = self
            .rows
            .iter()
            .map(|r| r.name.chars().count())
            .max()
            .unwrap_or(10)
            .clamp(10, 28);
        let kind_w = 17;
        let header = format!("  {:<name_w$}  {:<kind_w$}  Status", "Provider", "Kind");
        frame.render_widget(
            Paragraph::new(fit(&header, inner.width as usize)).style(theme.accent_style()),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let list_h = inner.height.saturating_sub(4) as usize;
        for (row, r) in self.rows.iter().enumerate().take(list_h) {
            let on = row == self.cursor;
            let base = if on { theme.selected() } else { theme.base() };
            let dot_color = if r.ready { theme.green } else { theme.yellow };
            let line = Line::from(vec![
                Span::styled(if on { "► " } else { "  " }, base),
                Span::styled(
                    format!("{:<name_w$}", fit(&safe_text(&r.name), name_w)),
                    base,
                ),
                Span::styled("  ", base),
                Span::styled(
                    format!("{:<kind_w$}", r.kind),
                    if on { base } else { theme.dim_style() },
                ),
                Span::styled("  ", base),
                Span::styled(if r.ready { "● " } else { "○ " }, base.fg(dot_color)),
                Span::styled(
                    fit(
                        &safe_text(&r.status),
                        (inner.width as usize).saturating_sub(name_w + kind_w + 9),
                    ),
                    base,
                ),
            ]);
            let fill = Rect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1);
            frame.render_widget(Paragraph::new(line).style(base), fill);
        }
        let notes = [
            "Enter: set the API key (or see how the Claude plan is used)",
            "d: remove the saved key    Esc: close",
        ];
        for (i, n) in notes.iter().enumerate() {
            let y = inner.y + inner.height.saturating_sub(2) + i as u16;
            frame.render_widget(
                Paragraph::new(fit(n, inner.width as usize)).style(theme.dim_style()),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
    }
}

impl ThemePicker {
    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.themes.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancel,
            KeyCode::Enter => {
                return match self.themes.get(self.cursor) {
                    Some(t) => Outcome::Submit(Submitted::Theme(t.id.clone())),
                    None => Outcome::Cancel,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.cursor = (self.cursor + 1).min(last),
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(self.page),
            KeyCode::PageDown => self.cursor = (self.cursor + self.page).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = last,
            _ => {}
        }
        Outcome::Pending
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let h = (self.themes.len() as u16 + 5).min(area.height);
        let inner = open(frame, area, 58.min(area.width), h, "Theme", theme);
        let inner = Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(1),
        );
        let list_h = inner.height.saturating_sub(2) as usize;
        self.page = list_h.max(1);
        let top = (self.cursor + 1).saturating_sub(list_h.max(1));
        for (row, t) in self.themes.iter().enumerate().skip(top).take(list_h) {
            let y = inner.y + (row - top) as u16;
            let on = row == self.cursor;
            let line_style = if on { theme.selected() } else { theme.base() };
            let mark = if t.id == self.current { "●" } else { " " };
            let name = fit(
                &format!("{}{}", t.name, if t.light { " (light)" } else { "" }),
                26,
            );
            let pointer = if on { "►" } else { " " };
            let mut spans = vec![Span::styled(
                format!("{pointer} {mark} {name:<26} "),
                line_style,
            )];
            // A swatch drawn in the candidate's own colors, so the row previews it.
            let on_theme = Style::default().bg(t.bg);
            spans.push(Span::styled(" Aa ", on_theme.fg(t.fg)));
            for c in [t.accent, t.accent2, t.green, t.red, t.yellow, t.cyan] {
                spans.push(Span::styled("██", on_theme.fg(c)));
            }
            spans.push(Span::styled(" ", on_theme));
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
        let hint = "Up/Down preview   Enter apply & save   Esc revert";
        frame.render_widget(
            Paragraph::new(fit(hint, inner.width as usize)).style(theme.dim_style()),
            Rect::new(
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width,
                1,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn typ(m: &mut Modal, s: &str) {
        for c in s.chars() {
            m.handle_key(key(KeyCode::Char(c)));
        }
    }
    fn models() -> Vec<ModelInfo> {
        ["openai/gpt-x", "anthropic/claude", "meta/llama"]
            .iter()
            .map(|id| ModelInfo::from_value(&json!({"id": id})))
            .collect()
    }

    #[test]
    fn form_requires_a_name_and_submits_both_fields() {
        let mut m = Modal::form(Purpose::NewNotebook, "New", "", "");
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Pending
        )); // moves to description
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Pending
        )); // empty name -> error, back to name
        typ(&mut m, "Notes");
        m.handle_key(key(KeyCode::Tab));
        typ(&mut m, "about things");
        match m.handle_key(key(KeyCode::Enter)) {
            Outcome::Submit(Submitted::Form { name, description }) => assert_eq!(
                (name.as_str(), description.as_str()),
                ("Notes", "about things")
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(m.handle_key(key(KeyCode::Esc)), Outcome::Cancel));
    }

    #[test]
    fn confirm_defaults_to_no() {
        let mut m = Modal::confirm(Purpose::ClearChat, "Sure?");
        assert!(matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Cancel));
        m.handle_key(key(KeyCode::Right));
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Submit(Submitted::Confirmed)
        ));
        assert!(matches!(
            m.handle_key(key(KeyCode::Char('y'))),
            Outcome::Submit(Submitted::Confirmed)
        ));
        assert!(matches!(
            m.handle_key(key(KeyCode::Char('n'))),
            Outcome::Cancel
        ));
    }

    #[test]
    fn input_returns_text_even_when_empty() {
        let mut m = Modal::input(Purpose::Filter, "Filter", "msg", false, "");
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Text(t)) if t.is_empty())
        );
        typ(&mut m, "abc");
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Text(t)) if t == "abc")
        );
    }

    #[test]
    fn picker_sorts_filters_and_chooses() {
        let mut m = Modal::picker(
            Purpose::NotebookModel,
            models(),
            Some("meta/llama".into()),
            false,
        );
        // sorted: anthropic, meta, openai -> the cursor starts on the current model
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Model { id, set_default: false }) if id == "meta/llama")
        );
        typ(&mut m, "GPT");
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Model { id, .. }) if id == "openai/gpt-x")
        );
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            m.handle_key(ctrl_d),
            Outcome::Submit(Submitted::Model {
                set_default: true,
                ..
            })
        ));
        typ(&mut m, "zzz"); // nothing matches: Enter does nothing
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Pending
        ));
    }

    #[test]
    fn default_picker_always_sets_default() {
        let mut m = Modal::picker(Purpose::DefaultModel, models(), None, true);
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Submit(Submitted::Model {
                set_default: true,
                ..
            })
        ));
    }

    #[test]
    fn text_view_closes_on_escape_and_sanitizes() {
        let mut m = Modal::text(Purpose::Info, "t\x1b", "a\x1b[2Jb");
        match &m.kind {
            Kind::Text(t) => assert_eq!((t.title.as_str(), t.text.as_str()), ("t", "a[2Jb")),
            _ => unreachable!(),
        }
        m.handle_key(key(KeyCode::PageDown));
        assert!(matches!(m.handle_key(key(KeyCode::Esc)), Outcome::Cancel));
    }

    #[test]
    fn theme_picker_previews_and_submits_the_highlighted_theme() {
        let themes = Theme::builtin();
        let second = themes[1].id.clone();
        let mut m = Modal::theme_picker(themes.clone(), &themes[0].id);
        assert_eq!(m.highlighted_theme().unwrap().id, themes[0].id);
        m.handle_key(key(KeyCode::Down));
        assert_eq!(m.highlighted_theme().unwrap().id, second);
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Theme(id)) if id == second)
        );
        assert!(matches!(m.handle_key(key(KeyCode::Esc)), Outcome::Cancel));
        m.handle_key(key(KeyCode::End));
        assert_eq!(m.highlighted_theme().unwrap().id, themes.last().unwrap().id);
        assert!(
            Modal::confirm(Purpose::ClearChat, "x")
                .highlighted_theme()
                .is_none()
        );
    }

    #[test]
    fn picker_keeps_same_named_models_from_different_providers_apart() {
        let mk = |provider: &str, id: &str| {
            ModelInfo::from_value(&json!({"id": id})).for_provider(provider)
        };
        let models = vec![
            mk("openrouter", "llama3"),
            mk("ollama", "llama3"),
            mk("anthropic", "claude"),
        ];
        let mut m = Modal::picker(
            Purpose::NotebookModel,
            models,
            Some("ollama::llama3".into()),
            false,
        );
        // Sorted by provider then id: anthropic, ollama, openrouter; the current model is preselected.
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Model { id, .. }) if id == "ollama::llama3")
        );
        typ(&mut m, "llama");
        m.handle_key(key(KeyCode::Down));
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Model { id, .. }) if id == "llama3"),
            "the OpenRouter one stays a bare id"
        );
        typ(&mut m, "anthr"); // typing "llamaanthr" matches nothing
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Pending
        ));
    }

    #[test]
    fn provider_list_selects_and_clears_keys() {
        use crate::llm::provider::ProviderKind;
        let providers = vec![
            ProviderConfig::builtin(ProviderKind::OpenRouter),
            ProviderConfig::builtin(ProviderKind::Anthropic),
        ];
        let mut m = Modal::providers(&providers);
        assert_eq!(m.purpose, Purpose::Providers);
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Provider(id)) if id == "openrouter")
        );
        m.handle_key(key(KeyCode::Down));
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Provider(id)) if id == "anthropic")
        );
        assert!(
            matches!(m.handle_key(key(KeyCode::Char('d'))), Outcome::Submit(Submitted::ClearKey(id)) if id == "anthropic")
        );
        m.handle_key(key(KeyCode::Down)); // clamps
        assert!(
            matches!(m.handle_key(key(KeyCode::Enter)), Outcome::Submit(Submitted::Provider(id)) if id == "anthropic")
        );
        assert!(matches!(m.handle_key(key(KeyCode::Esc)), Outcome::Cancel));
    }
}
