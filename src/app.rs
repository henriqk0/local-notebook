//! Application state and behaviour. Input handling and background results are plain
//! methods on `App`, so the whole app can be driven in tests without a terminal.
use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::widgets::TableState;
use tokio_util::sync::CancellationToken;

use crate::chat::{self, GenEvent, GenJob, GenOutcome};
use crate::config::{self, Config};
use crate::llm::models_cache::{ModelCache, ModelInfo};
use crate::llm::provider::{self, OPENROUTER, ProviderConfig, ProviderKind};
use crate::models::{Message, Notebook, Source};
use crate::sources::{process, ranger};
use crate::store::{SourceUpdate, Store};
use crate::ui::input::TextBuf;
use crate::ui::modal::{Modal, Outcome, Purpose, Submitted};
use crate::ui::screens;
use crate::ui::text::safe_text;
use crate::ui::theme::{self, Theme};

pub const MAX_IMPORT_FILES: usize = 100;
pub const MAX_PREVIEW_CHARS: usize = 20_000;
const TOAST_SECONDS: u64 = 5;

pub const HELP_TEXT: &str = "\
NOTEBOOK LIST
  n          new notebook
  e          edit notebook
  d          delete notebook (confirm)
  Enter      open notebook
  /          filter notebooks (Esc clears the filter)
  m          choose the default model
  t          choose a color theme
  Ctrl+K     providers and API keys
  q          quit

NOTEBOOK
  Tab        switch between sources and chat
  Ctrl+O     choose a model for this notebook
  Ctrl+L     clear chat (confirm)
  PgUp/PgDn  scroll the chat
  Esc        cancel generation / back to the notebook list
  F1         this help

  Sources (when focused)
  a          add source (ranger; falls back to a path prompt)
  Space      toggle enabled
  x          remove source (confirm)
  r          re-import from disk
  Enter      preview text

  Chat (when focused)
  Enter      send message
  Ctrl+J     newline (also Alt+Enter, Shift+Enter where supported)

ANYWHERE
  F2         toggle Markdown rendering of replies
  Ctrl+T     color theme picker (live preview; custom themes in
             ~/.config/rsnotebook/themes/*.toml)
  Ctrl+K     providers and API keys (OpenRouter, Anthropic API,
             Claude plan, custom OpenAI-compatible endpoints)
  Ctrl+C     quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

pub struct Toast {
    pub text: String,
    pub severity: Severity,
    until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sources,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Notebook,
}

pub struct ListView {
    /// Rows after filtering.
    pub rows: Vec<(Notebook, usize)>,
    pub table: TableState,
    pub filter: String,
}

pub struct NotebookView {
    pub id: i64,
    pub name: String,
    pub model: Option<String>,
    pub sources: Vec<Source>,
    pub src_table: TableState,
    pub messages: Vec<Message>,
    pub input: TextBuf,
    pub focus: Focus,
    /// Incremented per generation so late events from an older run are ignored.
    pub run: u64,
    pub generating: bool,
    /// The assistant text streamed so far in the current run.
    pub live: String,
    cancel: Option<CancellationToken>,
    /// Lines scrolled up from the bottom of the chat log (0 = pinned to the newest).
    pub scroll: usize,
    clear_when_done: bool,
    /// Number of imports in flight.
    pub importing: usize,
    /// Rendered chat history, reused between frames (see `screens::history_lines`).
    pub chat_cache: Option<screens::ChatCache>,
}

/// Results coming back from background work.
pub enum Bg {
    Models {
        purpose: Purpose,
        models: Vec<ModelInfo>,
        /// One entry per provider whose model list could not be loaded.
        warnings: Vec<String>,
    },
    Imported {
        nb: i64,
        added: usize,
        errors: usize,
    },
    Reimported {
        nb: i64,
        error: Option<String>,
    },
    Gen {
        nb: i64,
        run: u64,
        event: GenEvent,
    },
}

pub struct App {
    pub store: Store,
    pub cfg: Config,
    pub screen: Screen,
    pub modal: Option<Modal>,
    pub toast: Option<Toast>,
    pub list: ListView,
    pub nb: Option<NotebookView>,
    pub quit: bool,
    pub dirty: bool,
    pub tick: u64,
    want_ranger: bool,
    rt: tokio::runtime::Handle,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
    /// Where per-provider model caches live (`models_cache.json` is OpenRouter's).
    cache_dir: PathBuf,
    /// The theme being drawn with (may be a live preview), already adapted to the terminal.
    pub theme: Theme,
    /// All known themes in their original RGB form.
    themes: Vec<Theme>,
    /// The theme id saved in the config; what Esc in the theme picker returns to.
    committed_theme: String,
    pub truecolor: bool,
    /// Render assistant replies as Markdown (F2 toggles; saved in the config).
    pub markdown: bool,
}

fn move_selection(state: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let cur = state.selected().unwrap_or(0) as isize;
    state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
}

fn ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

impl App {
    pub fn new(store: Store, cfg: Config, rt: tokio::runtime::Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            store,
            cfg,
            screen: Screen::List,
            modal: None,
            toast: None,
            list: ListView {
                rows: Vec::new(),
                table: TableState::default(),
                filter: String::new(),
            },
            nb: None,
            quit: false,
            dirty: true,
            tick: 0,
            want_ranger: false,
            rt,
            tx,
            rx,
            cache_dir: config::data_dir(),
            theme: Theme::terminal(),
            themes: Vec::new(),
            committed_theme: String::new(),
            truecolor: theme::detect_truecolor(),
            markdown: true,
        };
        app.markdown = app.cfg.markdown;
        app.load_themes();
        app.refresh_notebooks();
        if !app.cfg.any_provider_ready() {
            app.notify("Set up a provider to start chatting.", Severity::Info);
            app.open_providers_dialog();
        }
        app
    }

    pub fn set_cache_dir(&mut self, dir: PathBuf) {
        self.cache_dir = dir;
    }

    fn load_themes(&mut self) {
        let (themes, problems) = theme::load_all(Some(&self.cfg.dir().join("themes")));
        self.themes = themes;
        let wanted = self
            .cfg
            .theme
            .clone()
            .unwrap_or_else(|| theme::DEFAULT_THEME.to_string());
        let id = if self.themes.iter().any(|t| t.id == wanted) {
            wanted
        } else {
            theme::DEFAULT_THEME.to_string()
        };
        self.committed_theme = id.clone();
        self.preview_theme(&id);
        if let Some(first) = problems.first() {
            let more = if problems.len() > 1 {
                format!(" (+{} more)", problems.len() - 1)
            } else {
                String::new()
            };
            self.notify(
                format!("Theme file skipped: {first}{more}"),
                Severity::Warning,
            );
        }
    }

    /// Adapt a theme to what the terminal can show.
    fn display_theme(&self, t: &Theme) -> Theme {
        if self.truecolor {
            t.clone()
        } else {
            t.clone().to_256()
        }
    }

    /// Switch the drawn theme without saving it. Returns false for an unknown id.
    fn preview_theme(&mut self, id: &str) -> bool {
        let Some(t) = self.themes.iter().find(|t| t.id == id).cloned() else {
            return false;
        };
        self.theme = self.display_theme(&t);
        self.dirty = true;
        true
    }

    fn toggle_markdown(&mut self) {
        self.markdown = !self.markdown;
        self.cfg.markdown = self.markdown;
        let state = if self.markdown { "on" } else { "off" };
        match self.cfg.save() {
            Ok(()) => self.notify(format!("Markdown rendering {state}"), Severity::Info),
            Err(e) => self.notify(
                format!("Markdown rendering {state} (could not save: {e})"),
                Severity::Warning,
            ),
        }
    }

    fn open_theme_picker(&mut self) {
        let themes: Vec<Theme> = self.themes.iter().map(|t| self.display_theme(t)).collect();
        self.modal = Some(Modal::theme_picker(themes, &self.committed_theme));
    }

    fn commit_theme(&mut self, id: String) {
        if !self.preview_theme(&id) {
            return;
        }
        self.committed_theme = id.clone();
        self.cfg.theme = Some(id);
        match self.cfg.save() {
            Ok(()) => self.notify(format!("Theme: {}", self.theme.name), Severity::Info),
            Err(e) => self.notify(format!("Could not save theme: {e}"), Severity::Error),
        }
    }

    pub fn notify(&mut self, text: impl Into<String>, severity: Severity) {
        self.toast = Some(Toast {
            text: text.into(),
            severity,
            until: Instant::now() + Duration::from_secs(TOAST_SECONDS),
        });
        self.dirty = true;
    }

    fn open_providers_dialog(&mut self) {
        self.modal = Some(Modal::providers(&self.cfg.providers));
    }

    /// Ask for a provider's API key, or explain how the Claude plan is used.
    fn open_provider_setup(&mut self, id: &str) {
        let Some(p) = self.cfg.provider(id) else {
            return;
        };
        if !p.uses_key() {
            let status = p.status();
            let text = format!(
                "Claude plan (claude CLI)\n\n\
                 Status: {status}\n\n\
                 rsnotebook runs the official `claude` CLI in headless mode, so your Pro/Max plan is used by \
                 Claude Code itself. No login token is read or copied.\n\n\
                 Setup: install Claude Code, then run `claude` once and sign in with your plan.\n\n\
                 Safety: tools, MCP servers and slash commands are switched off; it runs in an empty temporary \
                 directory; ANTHROPIC_API_KEY is removed from its environment so it can never bill the API \
                 instead of your plan.\n\n\
                 Answers count against your plan's usage limits. Models: default (your plan's choice), \
                 sonnet, opus, haiku."
            );
            self.modal = Some(Modal::text(Purpose::Info, "Claude plan", &text));
            return;
        }
        let where_to = match p.kind {
            ProviderKind::OpenRouter => "Get a key at openrouter.ai/keys.",
            ProviderKind::Anthropic => {
                "Create a key in the Anthropic Console. This is the pay-as-you-go API, separate from a Claude plan."
            }
            _ => "The key is optional for local servers.",
        };
        let env = p
            .api_key_env
            .as_deref()
            .map(|e| format!(" A key in ${e} takes precedence over the saved one."))
            .unwrap_or_default();
        let path = self.cfg.dir().join("config.toml");
        let msg = format!(
            "{where_to}{env} Saved to {} with mode 0600. Leave empty to keep the current key.",
            path.display()
        );
        self.modal = Some(Modal::input(
            Purpose::ProviderKey(id.to_string()),
            &format!("Key for {}", p.name),
            &msg,
            true,
            "",
        ));
    }

    fn save_config(&mut self, ok: String) {
        match self.cfg.save() {
            Ok(()) => self.notify(ok, Severity::Info),
            Err(e) => self.notify(format!("Could not save settings: {e}"), Severity::Error),
        }
    }

    // ---- notebook list ------------------------------------------------------

    pub fn refresh_notebooks(&mut self) {
        let rows = match self.store.list_notebooks_with_counts() {
            Ok(r) => r,
            Err(e) => {
                return self.notify(format!("Could not load notebooks: {e}"), Severity::Error);
            }
        };
        let needle = self.list.filter.to_lowercase();
        self.list.rows = rows
            .into_iter()
            .filter(|(nb, _)| {
                needle.is_empty()
                    || nb.name.to_lowercase().contains(&needle)
                    || nb.description.to_lowercase().contains(&needle)
            })
            .collect();
        let len = self.list.rows.len();
        let keep = self
            .list
            .table
            .selected()
            .unwrap_or(0)
            .min(len.saturating_sub(1));
        self.list.table.select((len > 0).then_some(keep));
        self.dirty = true;
    }

    pub fn selected_notebook(&self) -> Option<&Notebook> {
        self.list
            .rows
            .get(self.list.table.selected()?)
            .map(|(nb, _)| nb)
    }

    fn list_key(&mut self, key: KeyEvent) {
        let len = self.list.rows.len();
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help(),
            KeyCode::Char('n') => {
                self.modal = Some(Modal::form(Purpose::NewNotebook, "New Notebook", "", ""))
            }
            KeyCode::Char('e') => {
                if let Some(nb) = self.selected_notebook().cloned() {
                    self.modal = Some(Modal::form(
                        Purpose::EditNotebook(nb.id),
                        "Edit Notebook",
                        &nb.name,
                        &nb.description,
                    ));
                }
            }
            KeyCode::Char('d') => {
                if let Some(nb) = self.selected_notebook().cloned() {
                    self.modal = Some(Modal::confirm(
                        Purpose::DeleteNotebook(nb.id),
                        &format!("Delete notebook '{}'?", nb.name),
                    ));
                }
            }
            KeyCode::Enter => {
                if let Some(id) = self.selected_notebook().map(|nb| nb.id) {
                    self.open_notebook(id);
                }
            }
            KeyCode::Char('/') => {
                let initial = self.list.filter.clone();
                self.modal = Some(Modal::input(
                    Purpose::Filter,
                    "Filter",
                    "Filter notebooks (leave empty to clear):",
                    false,
                    &initial,
                ));
            }
            KeyCode::Esc if !self.list.filter.is_empty() => {
                self.list.filter.clear();
                self.refresh_notebooks();
            }
            KeyCode::Char('m') => self.request_models(Purpose::DefaultModel),
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Up | KeyCode::Char('k') => move_selection(&mut self.list.table, len, -1),
            KeyCode::Down | KeyCode::Char('j') => move_selection(&mut self.list.table, len, 1),
            KeyCode::PageUp => move_selection(&mut self.list.table, len, -10),
            KeyCode::PageDown => move_selection(&mut self.list.table, len, 10),
            KeyCode::Home | KeyCode::Char('g') => {
                move_selection(&mut self.list.table, len, -(len as isize))
            }
            KeyCode::End | KeyCode::Char('G') => {
                move_selection(&mut self.list.table, len, len as isize)
            }
            _ => {}
        }
    }

    fn show_help(&mut self) {
        self.modal = Some(Modal::text(Purpose::Info, "rsnotebook help", HELP_TEXT));
    }

    // ---- opening / leaving a notebook -----------------------------------------

    pub fn open_notebook(&mut self, id: i64) {
        let nb = match self.store.get_notebook(id) {
            Ok(Some(nb)) => nb,
            Ok(None) => return self.refresh_notebooks(),
            Err(e) => return self.notify(format!("Could not open notebook: {e}"), Severity::Error),
        };
        let messages = self.store.list_messages(id, Some(200)).unwrap_or_default();
        self.nb = Some(NotebookView {
            id,
            name: nb.name,
            model: nb.model,
            sources: Vec::new(),
            src_table: TableState::default(),
            messages,
            input: TextBuf::multiline(),
            focus: Focus::Chat,
            run: 0,
            generating: false,
            live: String::new(),
            cancel: None,
            scroll: 0,
            chat_cache: None,
            clear_when_done: false,
            importing: 0,
        });
        self.screen = Screen::Notebook;
        self.refresh_sources();
    }

    fn leave_notebook(&mut self) {
        self.nb = None;
        self.screen = Screen::List;
        self.refresh_notebooks();
    }

    pub fn refresh_sources(&mut self) {
        let Some(nb) = self.nb.as_mut() else { return };
        match self.store.list_source_meta(nb.id) {
            Ok(sources) => {
                let keep = nb
                    .src_table
                    .selected()
                    .unwrap_or(0)
                    .min(sources.len().saturating_sub(1));
                nb.src_table.select((!sources.is_empty()).then_some(keep));
                nb.sources = sources;
            }
            Err(e) => self.notify(format!("Could not load sources: {e}"), Severity::Error),
        }
        self.dirty = true;
    }

    fn selected_source(&self) -> Option<&Source> {
        let nb = self.nb.as_ref()?;
        nb.sources.get(nb.src_table.selected()?)
    }

    // ---- key handling ---------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.dirty = true;
        if let Some(modal) = self.modal.as_mut() {
            match modal.handle_key(key) {
                Outcome::Pending => {
                    // The theme picker previews whatever is highlighted.
                    if let Some(id) = modal.highlighted_theme().map(|t| t.id.clone()) {
                        self.preview_theme(&id);
                    }
                }
                Outcome::Cancel => {
                    if modal.purpose == Purpose::Theme {
                        let id = self.committed_theme.clone();
                        self.preview_theme(&id);
                    }
                    self.modal = None;
                }
                Outcome::Submit(value) => {
                    let purpose = modal.purpose.clone();
                    self.modal = None;
                    self.on_submit(purpose, value);
                }
            }
            return;
        }
        if ctrl(&key, 'c') {
            self.quit = true;
            return;
        }
        if ctrl(&key, 'k') {
            return self.open_providers_dialog();
        }
        if ctrl(&key, 't') {
            return self.open_theme_picker();
        }
        if key.code == KeyCode::F(2) {
            return self.toggle_markdown();
        }
        match self.screen {
            Screen::List => self.list_key(key),
            Screen::Notebook => self.notebook_key(key),
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        self.dirty = true;
        if let Some(m) = self.modal.as_mut() {
            m.paste(text);
        } else if let Some(nb) = self
            .nb
            .as_mut()
            .filter(|n| n.focus == Focus::Chat && !n.generating)
        {
            nb.input.insert_str(text);
        }
    }

    fn notebook_key(&mut self, key: KeyEvent) {
        let Some(nb) = self.nb.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                if nb.generating {
                    if let Some(c) = &nb.cancel {
                        c.cancel();
                    }
                    self.notify("Generation cancelled", Severity::Info);
                } else {
                    self.leave_notebook();
                }
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                nb.focus = if nb.focus == Focus::Chat {
                    Focus::Sources
                } else {
                    Focus::Chat
                };
                return;
            }
            KeyCode::F(1) => return self.show_help(),
            KeyCode::PageUp => {
                nb.scroll += 10;
                return;
            }
            KeyCode::PageDown => {
                nb.scroll = nb.scroll.saturating_sub(10);
                return;
            }
            _ => {}
        }
        if ctrl(&key, 'o') {
            return self.request_models(Purpose::NotebookModel);
        }
        if ctrl(&key, 'l') {
            self.modal = Some(Modal::confirm(
                Purpose::ClearChat,
                "Clear this notebook's chat history?",
            ));
            return;
        }
        match nb.focus {
            Focus::Sources => self.sources_key(key),
            Focus::Chat => self.chat_key(key),
        }
    }

    fn sources_key(&mut self, key: KeyEvent) {
        let len = self.nb.as_ref().map_or(0, |n| n.sources.len());
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(nb) = self.nb.as_mut() {
                    move_selection(&mut nb.src_table, len, -1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(nb) = self.nb.as_mut() {
                    move_selection(&mut nb.src_table, len, 1);
                }
            }
            KeyCode::Char('?') => self.show_help(),
            KeyCode::Char('a') => self.add_source(),
            KeyCode::Char(' ') => {
                if let Some((id, enabled)) = self.selected_source().map(|s| (s.id, s.enabled)) {
                    if let Err(e) = self.store.toggle_source(id, !enabled) {
                        self.notify(format!("Could not update source: {e}"), Severity::Error);
                    }
                    self.refresh_sources();
                }
            }
            KeyCode::Char('x') => {
                if let Some((id, name)) = self.selected_source().map(|s| (s.id, s.filename.clone()))
                {
                    self.modal = Some(Modal::confirm(
                        Purpose::RemoveSource(id),
                        &format!("Remove source '{name}'?"),
                    ));
                }
            }
            KeyCode::Char('r') => {
                if let Some(id) = self.selected_source().map(|s| s.id) {
                    self.reimport(id);
                }
            }
            KeyCode::Enter => self.preview_source(),
            _ => {}
        }
    }

    fn chat_key(&mut self, key: KeyEvent) {
        let Some(nb) = self.nb.as_mut() else { return };
        let newline = key.code == KeyCode::Enter
            && key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
            || ctrl(&key, 'j');
        if nb.generating {
            return; // the input is read-only while an answer streams
        }
        if newline {
            nb.input.insert_char('\n');
        } else if key.code == KeyCode::Enter {
            self.send_message();
        } else {
            nb.input.handle_key(key);
        }
    }

    // ---- modal results --------------------------------------------------------

    fn on_submit(&mut self, purpose: Purpose, value: Submitted) {
        match (purpose, value) {
            (Purpose::NewNotebook, Submitted::Form { name, description }) => {
                match self.store.create_notebook(name.trim(), description.trim()) {
                    Ok(_) => self.refresh_notebooks(),
                    Err(e) => {
                        self.notify(format!("Could not create notebook: {e}"), Severity::Error)
                    }
                }
            }
            (Purpose::EditNotebook(id), Submitted::Form { name, description }) => {
                match self.store.update_notebook(
                    id,
                    Some(name.trim()),
                    Some(description.trim()),
                    None,
                ) {
                    Ok(()) => self.refresh_notebooks(),
                    Err(e) => {
                        self.notify(format!("Could not update notebook: {e}"), Severity::Error)
                    }
                }
            }
            (Purpose::DeleteNotebook(id), Submitted::Confirmed) => match self
                .store
                .delete_notebook(id)
            {
                Ok(_) => self.refresh_notebooks(),
                Err(e) => self.notify(format!("Could not delete notebook: {e}"), Severity::Error),
            },
            (Purpose::Filter, Submitted::Text(t)) => {
                self.list.filter = t.trim().to_string();
                self.refresh_notebooks();
            }
            (Purpose::Providers, Submitted::Provider(id)) => self.open_provider_setup(&id),
            (Purpose::Providers, Submitted::ClearKey(id)) => {
                let name = self
                    .cfg
                    .provider(&id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                if self
                    .cfg
                    .provider(&id)
                    .is_some_and(|p| p.saved_key.is_some())
                {
                    self.cfg.clear_provider_key(&id);
                    self.save_config(format!("Removed the saved key for {name}"));
                } else {
                    self.notify(format!("{name} has no saved key"), Severity::Info);
                }
                self.open_providers_dialog();
            }
            (Purpose::ProviderKey(id), Submitted::Text(t)) => {
                let key = t.trim();
                if !key.is_empty() && self.cfg.set_provider_key(&id, key) {
                    let name = self
                        .cfg
                        .provider(&id)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    self.save_config(format!("Saved the key for {name}"));
                }
                self.open_providers_dialog();
            }
            (Purpose::AddPath, Submitted::Text(t)) => self.import_from_input(&t),
            (Purpose::RemoveSource(id), Submitted::Confirmed) => {
                if let Err(e) = self.store.delete_source(id) {
                    self.notify(format!("Could not remove source: {e}"), Severity::Error);
                }
                self.refresh_sources();
            }
            (Purpose::ClearChat, Submitted::Confirmed) => self.clear_chat(),
            (Purpose::Theme, Submitted::Theme(id)) => self.commit_theme(id),
            (Purpose::NotebookModel, Submitted::Model { id, set_default }) => {
                self.apply_notebook_model(id, set_default)
            }
            (Purpose::DefaultModel, Submitted::Model { id, .. }) => self.set_default_model(id),
            _ => {}
        }
    }

    fn set_default_model(&mut self, id: String) {
        self.cfg.default_model = Some(id.clone());
        match self.cfg.save() {
            Ok(()) => self.notify(
                format!("Default model set to {}", safe_text(&id)),
                Severity::Info,
            ),
            Err(e) => self.notify(
                format!("Could not save default model: {e}"),
                Severity::Error,
            ),
        }
    }

    fn apply_notebook_model(&mut self, id: String, set_default: bool) {
        let Some(nb) = self.nb.as_mut() else { return };
        nb.model = Some(id.clone());
        let nb_id = nb.id;
        if let Err(e) = self.store.update_notebook(nb_id, None, None, Some(&id)) {
            self.notify(format!("Could not save model: {e}"), Severity::Error);
        }
        if set_default {
            self.set_default_model(id);
        }
    }

    // ---- models ---------------------------------------------------------------

    /// Load the model list (cache first, then the API) in the background and open
    /// the picker when it arrives.
    fn request_models(&mut self, purpose: Purpose) {
        let tx = self.tx.clone();
        let dir = self.cache_dir.clone();
        let ready: Vec<ProviderConfig> = self
            .cfg
            .providers
            .iter()
            .filter(|p| p.is_ready())
            .cloned()
            .collect();
        self.rt.spawn(async move {
            let results =
                futures_util::future::join_all(ready.iter().map(|p| load_provider_models(p, &dir)))
                    .await;
            let (mut models, mut warnings) = (Vec::new(), Vec::new());
            for r in results {
                match r {
                    Ok(m) => models.extend(m),
                    Err(w) => warnings.push(w),
                }
            }
            let _ = tx.send(Bg::Models {
                purpose,
                models,
                warnings,
            });
        });
    }

    // ---- sources --------------------------------------------------------------

    fn add_source(&mut self) {
        if ranger::is_available(&self.cfg.ranger_command) {
            self.want_ranger = true;
        } else {
            self.open_path_dialog();
        }
    }

    fn open_path_dialog(&mut self) {
        self.modal = Some(Modal::input(
            Purpose::AddPath,
            "Add source",
            "Ranger unavailable or no selection. Enter a file path (or glob):",
            false,
            "",
        ));
    }

    pub fn take_ranger_request(&mut self) -> Option<String> {
        std::mem::take(&mut self.want_ranger).then(|| self.cfg.ranger_command.clone())
    }

    pub fn after_ranger(&mut self, paths: Vec<PathBuf>) {
        self.dirty = true;
        if paths.is_empty() {
            self.open_path_dialog();
        } else {
            self.import_paths(paths);
        }
    }

    pub fn import_from_input(&mut self, value: &str) {
        let value = value.trim();
        let expanded = process::expand_tilde(value);
        let mut paths: Vec<PathBuf> = glob::glob(&expanded.to_string_lossy())
            .map(|g| g.filter_map(Result::ok).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        if paths.is_empty() && expanded.is_file() {
            paths.push(expanded);
        }
        if paths.is_empty() {
            return self.notify(
                format!("No files matched: {}", safe_text(value)),
                Severity::Warning,
            );
        }
        self.import_paths(paths);
    }

    /// Extract and store files on a background thread.
    pub fn import_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(nb) = self.nb.as_mut() else { return };
        let mut files = ranger::dedupe_paths(paths.into_iter().filter(|p| p.is_file()).collect());
        if files.len() > MAX_IMPORT_FILES {
            files.truncate(MAX_IMPORT_FILES);
            self.toast = Some(Toast {
                text: format!("Only the first {MAX_IMPORT_FILES} files were imported."),
                severity: Severity::Warning,
                until: Instant::now() + Duration::from_secs(TOAST_SECONDS),
            });
        }
        nb.importing += 1;
        let (store, tx, nb_id) = (self.store.clone(), self.tx.clone(), nb.id);
        std::thread::spawn(move || {
            let (mut added, mut errors) = (0, 0);
            for path in files {
                let (source, chunks) = process::build_source_from_path(nb_id, &path);
                match store.add_source_with_chunks(&source, &chunks) {
                    Ok(_) if source.status == "error" => errors += 1,
                    Ok(_) => added += 1,
                    Err(_) => errors += 1,
                }
            }
            let _ = tx.send(Bg::Imported {
                nb: nb_id,
                added,
                errors,
            });
        });
    }

    fn reimport(&mut self, source_id: i64) {
        let Some(nb_id) = self.nb.as_ref().map(|n| n.id) else {
            return;
        };
        let (store, tx) = (self.store.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let error = reimport_source(&store, source_id);
            let _ = tx.send(Bg::Reimported { nb: nb_id, error });
        });
    }

    fn preview_source(&mut self) {
        let Some(source) = self.selected_source().cloned() else {
            return;
        };
        let full = self.store.get_source(source.id).ok().flatten();
        let mut text = full
            .as_ref()
            .map(|s| s.content_text.clone())
            .filter(|t| !t.is_empty())
            .or(source.error.clone())
            .unwrap_or_else(|| "(no content)".into());
        if text.chars().count() > MAX_PREVIEW_CHARS {
            text = text.chars().take(MAX_PREVIEW_CHARS).collect::<String>()
                + "\n\n... (preview truncated)";
        }
        self.modal = Some(Modal::text(Purpose::Info, &source.filename, &text));
    }

    // ---- chat -----------------------------------------------------------------

    fn send_message(&mut self) {
        let Some(nb) = self.nb.as_mut() else { return };
        let question = nb.input.text().trim().to_string();
        if question.is_empty() {
            return;
        }
        if nb.generating {
            return self.notify("Still generating. Press Esc to cancel.", Severity::Warning);
        }
        let prior = nb.messages.clone();
        match self
            .store
            .add_message(&Message::new(nb.id, "user", question.clone()))
        {
            Ok(saved) => nb.messages.push(saved),
            Err(e) => return self.notify(format!("Could not save message: {e}"), Severity::Error),
        }
        nb.input.clear();
        nb.generating = true;
        nb.live.clear();
        nb.scroll = 0;
        nb.run += 1;
        let cancel = CancellationToken::new();
        nb.cancel = Some(cancel.clone());

        let (nb_id, run) = (nb.id, nb.run);
        let job = GenJob {
            store: self.store.clone(),
            notebook_id: nb_id,
            question,
            prior,
            model: nb.model.clone().or_else(|| self.cfg.default_model.clone()),
            providers: self.cfg.providers.clone(),
            system_prompt: self.cfg.system_prompt.clone(),
            token_budget: self.cfg.context_token_budget,
            cancel,
        };
        let tx = self.tx.clone();
        let emit_tx = tx.clone();
        let task = self.rt.spawn(chat::run(job, move |event| {
            let _ = emit_tx.send(Bg::Gen {
                nb: nb_id,
                run,
                event,
            });
        }));
        // If the task panics, `Done` would never arrive and the input would stay locked.
        self.rt.spawn(async move {
            if task.await.is_err() {
                let outcome = GenOutcome {
                    saved: Vec::new(),
                    transient_error: Some("Internal error while generating the answer.".into()),
                };
                let _ = tx.send(Bg::Gen {
                    nb: nb_id,
                    run,
                    event: GenEvent::Done(outcome),
                });
            }
        });
    }

    fn clear_chat(&mut self) {
        let Some(nb) = self.nb.as_mut() else { return };
        if nb.generating {
            // Let the run finish (and save its partial answer) first, then clear.
            nb.clear_when_done = true;
            if let Some(c) = &nb.cancel {
                c.cancel();
            }
            return;
        }
        self.finish_clear();
    }

    fn finish_clear(&mut self) {
        let Some(nb) = self.nb.as_mut() else { return };
        if let Err(e) = self.store.clear_messages(nb.id) {
            return self.notify(format!("Could not clear chat: {e}"), Severity::Error);
        }
        nb.messages.clear();
        nb.scroll = 0;
        nb.clear_when_done = false;
    }

    fn finish_generation(&mut self, outcome: GenOutcome) {
        let Some(nb) = self.nb.as_mut() else { return };
        nb.messages.extend(outcome.saved);
        if let Some(text) = outcome.transient_error {
            nb.messages.push(Message::new(nb.id, "error", text));
        }
        nb.live.clear();
        nb.generating = false;
        nb.cancel = None;
        if nb.clear_when_done {
            self.finish_clear();
        }
    }

    // ---- background results & time --------------------------------------------

    pub fn drain_bg(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.dirty = true;
            self.on_bg(ev);
        }
    }

    pub fn on_bg(&mut self, ev: Bg) {
        match ev {
            Bg::Models {
                purpose,
                models,
                warnings,
            } => {
                if let Some(w) = warnings.first() {
                    let more = if warnings.len() > 1 {
                        format!(" (+{} more)", warnings.len() - 1)
                    } else {
                        String::new()
                    };
                    self.notify(
                        format!("Could not load models: {w}{more}"),
                        Severity::Warning,
                    );
                }
                if models.is_empty() {
                    if warnings.is_empty() {
                        self.notify(
                            "No models available. Press Ctrl+K to set up a provider.",
                            Severity::Warning,
                        );
                    }
                    return;
                }
                if self.modal.is_some() {
                    return;
                }
                match (purpose, self.nb.as_ref()) {
                    (Purpose::NotebookModel, Some(nb)) => {
                        let current = nb.model.clone().or_else(|| self.cfg.default_model.clone());
                        self.modal = Some(Modal::picker(
                            Purpose::NotebookModel,
                            models,
                            current,
                            false,
                        ));
                    }
                    (Purpose::DefaultModel, _) => {
                        self.modal = Some(Modal::picker(
                            Purpose::DefaultModel,
                            models,
                            self.cfg.default_model.clone(),
                            true,
                        ));
                    }
                    _ => {}
                }
            }
            Bg::Imported { nb, added, errors } => {
                if let Some(view) = self.nb.as_mut().filter(|v| v.id == nb) {
                    view.importing = view.importing.saturating_sub(1);
                }
                self.refresh_sources();
                match (added, errors) {
                    (0, 0) => {}
                    (0, e) => self.notify(format!("Could not import {e} file(s)"), Severity::Error),
                    (a, 0) => self.notify(format!("Added {a} source(s)"), Severity::Info),
                    (a, e) => self.notify(
                        format!("Added {a} source(s), {e} failed"),
                        Severity::Warning,
                    ),
                }
            }
            Bg::Reimported { nb, error } => {
                if self.nb.as_ref().is_some_and(|v| v.id == nb) {
                    self.refresh_sources();
                }
                if let Some(e) = error {
                    self.notify(e, Severity::Error);
                }
            }
            Bg::Gen { nb, run, event } => {
                if !self.nb.as_ref().is_some_and(|v| v.id == nb && v.run == run) {
                    return; // an older run, or the notebook was closed
                }
                match event {
                    GenEvent::Token(t) => {
                        if let Some(v) = self.nb.as_mut() {
                            v.live.push_str(&t);
                        }
                    }
                    GenEvent::Done(outcome) => self.finish_generation(outcome),
                }
            }
        }
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self
            .toast
            .as_ref()
            .is_some_and(|t| Instant::now() >= t.until)
        {
            self.toast = None;
            self.dirty = true;
        }
        if self
            .nb
            .as_ref()
            .is_some_and(|n| n.generating || n.importing > 0)
        {
            self.dirty = true; // animate the spinner
        }
    }
}

/// One provider's model list: from the on-disk cache when fresh (OpenRouter and Anthropic,
/// whose lists are large and change slowly), otherwise from the provider itself.
async fn load_provider_models(
    p: &ProviderConfig,
    dir: &std::path::Path,
) -> Result<Vec<ModelInfo>, String> {
    let cached = matches!(p.kind, ProviderKind::OpenRouter | ProviderKind::Anthropic);
    let file = if p.id == OPENROUTER {
        "models_cache.json".to_string()
    } else {
        format!("models_cache_{}.json", p.id)
    };
    let cache = ModelCache::new(dir.join(file));
    if cached {
        let hit = cache.load();
        if !hit.is_empty() {
            return Ok(hit.into_iter().map(|m| m.for_provider(&p.id)).collect());
        }
    }
    let models = provider::list_models(p)
        .await
        .map_err(|e| format!("{}: {}", p.name, e.message))?;
    if cached {
        let _ = cache.save(&models); // best effort: a read-only data dir must not break chat
    }
    Ok(models)
}

/// Re-extract a source from disk. Returns an error message to show, if any.
fn reimport_source(store: &Store, id: i64) -> Option<String> {
    let full = match store.get_source(id) {
        Ok(Some(s)) => s,
        _ => return None,
    };
    let path = PathBuf::from(&full.path);
    if !path.is_file() {
        let _ = store.mark_source_error(id, "File no longer exists");
        return Some(format!("File missing: {}", safe_text(&full.path)));
    }
    let ex = crate::sources::extract::extract_text(&path);
    let error = ex.error.or_else(|| {
        ex.content
            .trim()
            .is_empty()
            .then(|| process::NO_TEXT_ERROR.to_string())
    });
    let chunks = if error.is_none() {
        crate::sources::chunk::chunk_text(&ex.content)
    } else {
        Vec::new()
    };
    let update = SourceUpdate {
        token_estimate: crate::tokens::estimate(&ex.content),
        content_text: ex.content,
        size: ex.size,
        mime: ex.mime.map(String::from),
        ext: ex.ext,
        filename: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(full.filename),
        error: error.clone(),
    };
    match store.replace_source_content(id, &update, &chunks) {
        Ok(()) => error.map(|e| format!("Re-import failed: {e}")),
        Err(e) => Some(format!("Re-import failed: {e}")),
    }
}

// ---- terminal -----------------------------------------------------------------

pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen);
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout()))?,
        })
    }

    /// Hand the real terminal to a child process (ranger).
    fn suspend(&mut self) {
        restore_terminal();
    }

    fn resume(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Clear(ClearType::All)
        )?;
        // A fresh Terminal repaints everything and, unlike `Terminal::clear`, never
        // asks the terminal for the cursor position (not every terminal answers).
        self.terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        Ok(())
    }
}

/// Run the interactive app until the user quits.
pub fn run(store: Store, cfg: Config) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut app = App::new(store, cfg, runtime.handle().clone());
    let mut tui = Tui::enter()?;
    let result = event_loop(&mut app, &mut tui);
    restore_terminal();
    // Don't wait for a stalled network request on the way out.
    runtime.shutdown_timeout(Duration::from_millis(200));
    result
}

fn event_loop(app: &mut App, tui: &mut Tui) -> Result<()> {
    loop {
        if app.dirty {
            app.dirty = false;
            tui.terminal.draw(|f| screens::draw(app, f))?;
        }
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(33))? {
            loop {
                match event::read()? {
                    Event::Key(k) => app.on_key(k),
                    Event::Paste(s) => app.on_paste(&s),
                    Event::Resize(..) => app.dirty = true,
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        } else {
            app.on_tick();
        }
        app.drain_bg();
        if let Some(cmd) = app.take_ranger_request() {
            tui.suspend();
            let paths = ranger::pick_files(&cmd);
            tui.resume()?;
            app.after_ranger(paths);
        }
    }
}
