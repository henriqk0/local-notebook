//! Drives the whole app through key presses and renders to an in-memory terminal.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rsnotebook::app::{App, Focus, Screen};
use rsnotebook::config::Config;
use rsnotebook::store::Store;
use rsnotebook::ui::screens;

struct Harness {
    app: App,
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

fn harness_with_key(key: Option<&str>) -> Harness {
    harness_setup(key, |_| {})
}

fn harness_setup(key: Option<&str>, setup: impl FnOnce(&std::path::Path)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    // Built from the file only (never the environment) so a developer's own keys or an
    // installed `claude` cannot change what the tests see.
    let path = dir.path().join("config.toml");
    let table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| raw.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let mut cfg = Config::from_table(&table, path);
    cfg.provider_mut("claude-cli").unwrap().command = "no-claude".into();
    if let Some(k) = key {
        cfg.set_provider_key("openrouter", k);
    }
    let store = Store::open_in_memory().unwrap();
    let mut app = App::new(store, cfg, rt.handle().clone());
    app.set_cache_dir(dir.path().to_path_buf());
    Harness {
        app,
        _rt: rt,
        _dir: dir,
    }
}

fn set_openrouter_url(h: &mut Harness, url: String) {
    h.app.cfg.provider_mut("openrouter").unwrap().base_url = Some(url);
}

fn set_provider(
    h: &mut Harness,
    id: &str,
    f: impl FnOnce(&mut rsnotebook::llm::provider::ProviderConfig),
) {
    f(h.app.cfg.provider_mut(id).unwrap());
}

fn harness() -> Harness {
    harness_with_key(Some("test-key"))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

impl Harness {
    fn press(&mut self, code: KeyCode) {
        self.app.on_key(key(code));
    }
    fn ctrl(&mut self, c: char) {
        self.app
            .on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    fn type_str(&mut self, s: &str) {
        for c in s.chars() {
            self.press(KeyCode::Char(c));
        }
    }
    fn screen_buffer(&mut self) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| screens::draw(&mut self.app, f)).unwrap();
        term.backend().buffer().clone()
    }
    fn screen_text(&mut self) -> String {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| screens::draw(&mut self.app, f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn wait_for(&mut self, what: &str, mut cond: impl FnMut(&mut App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.app.drain_bg();
            if cond(&mut self.app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}\n{}", self.screen_text());
    }
    fn create_notebook(&mut self, name: &str, desc: &str) {
        self.press(KeyCode::Char('n'));
        self.type_str(name);
        self.press(KeyCode::Tab);
        self.type_str(desc);
        self.press(KeyCode::Enter);
    }
}

/// Serve canned SSE bodies, one per connection, on a local port.
fn fake_openrouter(bodies: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for body in bodies {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if sock.read_exact(&mut b).is_err() {
                    return;
                }
                req.push(b[0]);
            }
            let head = String::from_utf8_lossy(&req).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut buf = vec![0u8; len];
            let _ = sock.read_exact(&mut buf);
            let path_is_models = head.starts_with("get ");
            let ctype = if path_is_models {
                "application/json"
            } else {
                "text/event-stream"
            };
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    url
}

#[test]
fn notebook_crud_and_filter_from_the_keyboard() {
    let mut h = harness();
    assert!(
        h.screen_text()
            .contains("No notebooks yet. Press 'n' to create one.")
    );

    h.create_notebook("Research", "papers");
    h.create_notebook("Recipes", "food");
    let s = h.screen_text();
    assert!(s.contains("Research") && s.contains("Recipes") && s.contains("2 notebook(s)"));

    // Empty name is rejected and the dialog stays open.
    h.press(KeyCode::Char('n'));
    h.press(KeyCode::Enter);
    h.press(KeyCode::Enter);
    assert!(h.screen_text().contains("Name is required."));
    h.press(KeyCode::Esc);

    h.press(KeyCode::Char('/'));
    h.type_str("food");
    h.press(KeyCode::Enter);
    let s = h.screen_text();
    assert!(s.contains("Recipes") && !s.contains("Research") && s.contains("filter: food"));
    h.press(KeyCode::Esc);
    assert!(h.screen_text().contains("Research"));

    // Edit the selected notebook (the most recently touched one is first).
    h.press(KeyCode::Char('e'));
    h.type_str("!");
    h.press(KeyCode::Enter);
    h.press(KeyCode::Enter);
    assert!(h.screen_text().contains('!'));

    // Delete needs confirmation; Enter defaults to "No".
    h.press(KeyCode::Char('d'));
    h.press(KeyCode::Enter);
    assert_eq!(h.app.list.rows.len(), 2);
    h.press(KeyCode::Char('d'));
    h.press(KeyCode::Char('y'));
    assert_eq!(h.app.list.rows.len(), 1);
}

#[test]
fn hostile_names_cannot_crash_or_inject_escapes() {
    let mut h = harness();
    h.create_notebook("[/x] [bold]name", "desc");
    h.app
        .store
        .create_notebook("evil\u{1b}[2Jname", "")
        .unwrap();
    h.app.refresh_notebooks();
    let s = h.screen_text();
    assert!(s.contains("[/x] [bold]name"));
    assert!(!s.contains('\u{1b}'));
}

#[test]
fn startup_opens_the_providers_dialog_when_nothing_is_set_up() {
    let mut h = harness_with_key(None);
    let screen = h.screen_text();
    assert!(
        screen.contains("Providers")
            && screen.contains("OpenRouter")
            && screen.contains("Anthropic API")
            && screen.contains("Claude plan"),
        "{screen}"
    );
    assert!(
        screen.contains("needs an API key") && screen.contains("not found in PATH"),
        "{screen}"
    );

    // Enter on OpenRouter asks for its key, masked.
    h.press(KeyCode::Enter);
    assert!(h.screen_text().contains("Key for OpenRouter"));
    h.type_str("sk-or-secret-123");
    assert!(
        !h.screen_text().contains("sk-or-secret-123"),
        "key must be masked on screen"
    );
    h.press(KeyCode::Enter);
    assert_eq!(
        h.app.cfg.provider("openrouter").unwrap().api_key.as_deref(),
        Some("sk-or-secret-123")
    );
    // Back on the list, now showing the provider as ready.
    assert!(h.screen_text().contains("Providers") && h.screen_text().contains("ready"));
    h.press(KeyCode::Esc);
    assert!(h.app.modal.is_none());
    let saved = std::fs::read_to_string(h.app.cfg.dir().join("config.toml")).unwrap();
    assert!(
        saved.contains("api_key = \"sk-or-secret-123\""),
        "OpenRouter's key keeps the original top-level layout\n{saved}"
    );
}

#[test]
fn providers_dialog_sets_clears_and_explains_each_provider() {
    let mut h = harness();
    h.ctrl('k');
    assert!(h.screen_text().contains("Providers"));

    // Anthropic API: set a key, then remove it.
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert!(
        h.screen_text().contains("Key for Anthropic API")
            && h.screen_text().contains("pay-as-you-go")
    );
    h.type_str("sk-ant-abc");
    h.press(KeyCode::Enter);
    assert!(h.app.cfg.provider("anthropic").unwrap().is_ready());
    let saved = std::fs::read_to_string(h.app.cfg.dir().join("config.toml")).unwrap();
    assert!(
        saved.contains("[providers.anthropic]") && saved.contains("sk-ant-abc"),
        "{saved}"
    );
    h.press(KeyCode::Down); // dialog reopens at the top
    h.press(KeyCode::Char('d'));
    assert!(!h.app.cfg.provider("anthropic").unwrap().is_ready());
    assert!(
        !std::fs::read_to_string(h.app.cfg.dir().join("config.toml"))
            .unwrap()
            .contains("sk-ant-abc")
    );

    // Claude plan: no key is asked for; the dialog explains how it works.
    h.press(KeyCode::Down);
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    let s = h.screen_text();
    assert!(
        s.contains("headless")
            && s.contains("No login token is read or copied")
            && s.contains("tools"),
        "{s}"
    );
}

#[test]
fn source_management_in_a_notebook() {
    let mut h = harness();
    h.create_notebook("N", "");
    h.press(KeyCode::Enter);
    assert_eq!(h.app.screen, Screen::Notebook);
    assert!(h.screen_text().contains("No sources yet"));

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("notes.md");
    std::fs::write(&f, "Alpha beta gamma.\n\nSecond paragraph.").unwrap();
    std::fs::write(dir.path().join("empty.txt"), "  ").unwrap();
    std::fs::write(dir.path().join("pic.png"), [0u8, 1, 2]).unwrap();

    h.press(KeyCode::Tab); // focus sources
    h.app
        .import_from_input(&format!("{}/*", dir.path().display()));
    h.wait_for("import to finish", |a| {
        a.nb.as_ref()
            .is_some_and(|n| n.importing == 0 && n.sources.len() == 3)
    });
    let s = h.screen_text();
    assert!(s.contains("notes.md") && s.contains("err"), "{s}");
    assert_eq!(
        h.app
            .nb
            .as_ref()
            .unwrap()
            .sources
            .iter()
            .filter(|s| s.enabled)
            .count(),
        1
    );

    // Preview the first source (sorted by insertion: empty.txt, notes.md, pic.png via glob order).
    let idx = h
        .app
        .nb
        .as_ref()
        .unwrap()
        .sources
        .iter()
        .position(|s| s.filename == "notes.md")
        .unwrap();
    for _ in 0..idx {
        h.press(KeyCode::Down);
    }
    h.press(KeyCode::Enter);
    assert!(h.screen_text().contains("Alpha beta gamma."));
    h.press(KeyCode::Esc);

    // Toggle off and on.
    h.press(KeyCode::Char(' '));
    assert!(
        h.app
            .nb
            .as_ref()
            .unwrap()
            .sources
            .iter()
            .all(|s| !s.enabled)
    );
    h.press(KeyCode::Char(' '));
    assert!(h.app.nb.as_ref().unwrap().sources.iter().any(|s| s.enabled));

    // Re-import picks up edits on disk.
    std::fs::write(&f, "Completely new text.").unwrap();
    h.press(KeyCode::Char('r'));
    h.wait_for("reimport", |a| {
        let id = a.nb.as_ref().unwrap().sources[idx].id;
        a.store.get_source(id).unwrap().unwrap().content_text == "Completely new text."
    });

    // Remove with confirmation.
    let before = h.app.nb.as_ref().unwrap().sources.len();
    h.press(KeyCode::Char('x'));
    h.press(KeyCode::Char('y'));
    assert_eq!(h.app.nb.as_ref().unwrap().sources.len(), before - 1);

    // A vanished file marks the source as broken on re-import.
    let gone = dir.path().join("gone.txt");
    std::fs::write(&gone, "soon deleted").unwrap();
    h.app.import_from_input(&gone.to_string_lossy());
    h.wait_for("second import", |a| {
        a.nb.as_ref()
            .is_some_and(|n| n.importing == 0 && n.sources.iter().any(|s| s.filename == "gone.txt"))
    });
    std::fs::remove_file(&gone).unwrap();
    let idx = h
        .app
        .nb
        .as_ref()
        .unwrap()
        .sources
        .iter()
        .position(|s| s.filename == "gone.txt")
        .unwrap();
    h.app.nb.as_mut().unwrap().src_table.select(Some(idx));
    h.press(KeyCode::Char('r'));
    h.wait_for("missing file flagged", |a| {
        let s = &a.nb.as_ref().unwrap().sources;
        s.iter()
            .any(|s| s.filename == "gone.txt" && s.status == "error" && !s.enabled)
    });
    assert!(h.screen_text().contains("File missing"));
}

#[test]
fn chat_streams_persists_and_survives_reopening() {
    let mut h = harness();
    set_openrouter_url(&mut h, fake_openrouter(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"The sky \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"is blue.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into(),
    ]));
    h.create_notebook("N", "");
    h.press(KeyCode::Enter);

    // No model yet: the error is shown and stored.
    h.type_str("hello");
    h.press(KeyCode::Enter);
    h.wait_for("error reply", |a| {
        a.nb.as_ref()
            .unwrap()
            .messages
            .iter()
            .any(|m| m.role == "error")
    });
    assert!(h.screen_text().contains("No model selected"));

    h.app.cfg.default_model = Some("m/x".into());
    h.type_str("What colour is the sky?");
    h.press(KeyCode::Enter);
    assert!(h.app.nb.as_ref().unwrap().generating);
    h.wait_for("answer", |a| !a.nb.as_ref().unwrap().generating);
    let s = h.screen_text();
    assert!(
        s.contains("You ► What colour is the sky?") && s.contains("AI  ◄ The sky is blue."),
        "{s}"
    );

    // Back to the list and in again: history comes from the database.
    h.press(KeyCode::Esc);
    assert_eq!(h.app.screen, Screen::List);
    h.press(KeyCode::Enter);
    let msgs = &h.app.nb.as_ref().unwrap().messages;
    assert_eq!(
        msgs.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
        ["user", "error", "user", "assistant"]
    );

    // Clear chat needs confirmation.
    h.ctrl('l');
    h.press(KeyCode::Char('y'));
    assert!(h.app.nb.as_ref().unwrap().messages.is_empty());
    assert!(
        h.app
            .store
            .list_messages(h.app.nb.as_ref().unwrap().id, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn escape_cancels_generation_and_keeps_the_partial_answer() {
    let mut h = harness();
    // A server that sends one token and then stalls.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    set_openrouter_url(&mut h, format!("http://{}", listener.local_addr().unwrap()));
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut b = [0u8; 1];
        while !req.ends_with(b"\r\n\r\n") {
            sock.read_exact(&mut b).unwrap();
            req.push(b[0]);
        }
        let head = String::from_utf8_lossy(&req).to_lowercase();
        let len: usize = head
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut buf = vec![0u8; len];
        sock.read_exact(&mut buf).unwrap();
        sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial text\"}}]}\n\n").unwrap();
        std::thread::sleep(Duration::from_secs(5));
    });
    h.app.cfg.default_model = Some("m/x".into());
    h.create_notebook("N", "");
    h.press(KeyCode::Enter);
    h.type_str("go");
    h.press(KeyCode::Enter);
    h.wait_for("first token", |a| {
        a.nb.as_ref().unwrap().live.contains("partial text")
    });
    assert!(h.screen_text().contains("generating"));

    // Cancelling even leaves the screen usable: Esc again goes back once the run ends.
    h.press(KeyCode::Esc);
    h.wait_for("run to end", |a| !a.nb.as_ref().unwrap().generating);
    let msgs = &h.app.nb.as_ref().unwrap().messages;
    assert_eq!(
        msgs.last().unwrap().content,
        "partial text\n\n[interrupted]"
    );
    h.press(KeyCode::Esc);
    assert_eq!(h.app.screen, Screen::List);
}

#[test]
fn model_picker_sets_the_notebook_model_and_the_default() {
    let mut h = harness();
    set_openrouter_url(&mut h, fake_openrouter(vec![
        r#"{"data":[{"id":"zeta/model","name":"Zeta","context_length":8000,"pricing":{"prompt":"0.000001","completion":"0.000002"}},{"id":"alpha/free","pricing":{"prompt":"0","completion":"0"}}]}"#.into(),
    ]));
    h.create_notebook("N", "");
    h.press(KeyCode::Enter);
    h.ctrl('o');
    h.wait_for("picker", |a| a.modal.is_some());
    let s = h.screen_text();
    assert!(
        s.contains("alpha/free (free)") && s.contains("zeta/model"),
        "{s}"
    );
    h.type_str("zeta");
    h.ctrl('d'); // use and set as default
    assert_eq!(
        h.app.nb.as_ref().unwrap().model.as_deref(),
        Some("zeta/model")
    );
    assert_eq!(h.app.cfg.default_model.as_deref(), Some("zeta/model"));
    assert!(h.screen_text().contains("Default model set to zeta/model"));

    // The second open is served from the on-disk cache (the fake server only answers once).
    h.ctrl('o');
    h.wait_for("cached picker", |a| a.modal.is_some());
    h.press(KeyCode::Esc);
    assert!(h.app.modal.is_none());
}

#[test]
fn help_and_small_terminal_and_focus() {
    let mut h = harness();
    h.press(KeyCode::F(1));
    assert!(h.screen_text().contains("NOTEBOOK LIST"));
    h.press(KeyCode::Esc);

    h.create_notebook("N", "");
    h.press(KeyCode::Enter);
    assert_eq!(h.app.nb.as_ref().unwrap().focus, Focus::Chat);
    h.press(KeyCode::Tab);
    assert_eq!(h.app.nb.as_ref().unwrap().focus, Focus::Sources);
    // Single-letter keys type into the chat only when it is focused.
    h.press(KeyCode::Char('x'));
    h.press(KeyCode::Tab);
    h.type_str("x");
    assert_eq!(h.app.nb.as_ref().unwrap().input.text(), "x");

    let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
    term.draw(|f| screens::draw(&mut h.app, f)).unwrap();
    assert!(format!("{:?}", term.backend().buffer()).contains("Terminal too small"));
}

#[test]
fn borders_are_joined_with_box_drawing_junctions() {
    let mut h = harness();
    let rows = |t: &str| t.lines().map(String::from).collect::<Vec<_>>();

    // List: column dividers meet the top border, the header rule and the bottom border.
    h.create_notebook("Alpha", "first");
    let list = rows(&h.screen_text());
    assert!(
        list.iter()
            .any(|l| l.starts_with('┌') && l.contains('┬') && l.ends_with('┐')),
        "{list:#?}"
    );
    assert!(
        list.iter()
            .any(|l| l.starts_with('├') && l.contains('┼') && l.ends_with('┤'))
    );
    assert!(
        list.iter()
            .any(|l| l.starts_with('└') && l.contains('┴') && l.ends_with('┘'))
    );

    // Notebook: panes share borders, so there is no `││` pair and no unjoined `+`/`-` ASCII frame.
    h.press(KeyCode::Enter);
    let nb = h.screen_text();
    let r = rows(&nb);
    assert!(
        r.iter().any(|l| l.starts_with('┌') && l.contains('┬')),
        "{nb}"
    );
    assert!(
        r.iter()
            .any(|l| l.contains('├') && l.contains('┤') && l.starts_with('│')),
        "chat/input divider joins the shared border\n{nb}"
    );
    assert!(r.iter().any(|l| l.starts_with('└') && l.contains('┴')));
    assert!(
        !nb.contains("││") && !nb.contains("++") && !nb.contains("+-"),
        "{nb}"
    );
}

#[test]
fn theme_picker_previews_reverts_commits_and_persists() {
    let mut h = harness();
    let default_id = h.app.theme.id.clone();
    assert_eq!(default_id, "tokyo-night");
    let bg = h.app.theme.bg;
    assert_eq!(
        h.screen_buffer()[(3, 5)].bg,
        bg,
        "the whole screen is filled with the theme background"
    );

    // Esc reverts the live preview.
    h.ctrl('t');
    assert!(h.screen_text().contains("Theme") && h.screen_text().contains("Dracula"));
    h.press(KeyCode::Down);
    assert_eq!(h.app.theme.id, "dracula");
    assert_ne!(
        h.screen_buffer()[(3, 5)].bg,
        bg,
        "preview recolors immediately"
    );
    h.press(KeyCode::Esc);
    assert_eq!(h.app.theme.id, default_id);

    // Enter commits and saves.
    h.ctrl('t');
    h.press(KeyCode::Down);
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert_eq!(h.app.theme.id, "nord");
    assert!(h.screen_text().contains("Theme: Nord"));
    let cfg_path = h.app.cfg.dir().join("config.toml");
    assert!(
        std::fs::read_to_string(&cfg_path)
            .unwrap()
            .contains("theme = \"nord\"")
    );

    // A fresh app reading that config starts in the saved theme; from the notebook screen too.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let cfg = Config::load(Some(cfg_path));
    let mut again = App::new(Store::open_in_memory().unwrap(), cfg, rt.handle().clone());
    assert_eq!(again.theme.id, "nord");
    again.on_key(key(KeyCode::Char('t')));
    again.on_key(key(KeyCode::Esc));
    assert_eq!(again.theme.id, "nord");
}

#[test]
fn custom_themes_load_from_the_config_dir_and_bad_ones_are_reported() {
    let mut h = harness_setup(Some("k"), |dir| {
        let themes = dir.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(
            themes.join("neon.toml"),
            "name = \"Neon\"\nbase = \"dracula\"\naccent = \"#00ffcc\"\n",
        )
        .unwrap();
        std::fs::write(themes.join("broken.toml"), "bg = \"not-a-color\"").unwrap();
        std::fs::write(dir.join("config.toml"), "theme = \"neon\"\n").unwrap();
    });
    assert_eq!(h.app.theme.name, "Neon");
    assert_eq!(
        h.app.theme.accent,
        ratatui::style::Color::Rgb(0, 0xff, 0xcc)
    );
    assert!(
        h.screen_text().contains("broken.toml"),
        "the skipped file is reported"
    );
    h.press(KeyCode::Char('t'));
    assert!(h.screen_text().contains("Neon"));

    // An unknown theme id in the config falls back to the default instead of failing.
    let h2 = harness_setup(Some("k"), |dir| {
        std::fs::write(dir.join("config.toml"), "theme = \"does-not-exist\"\n").unwrap()
    });
    assert_eq!(h2.app.theme.id, "tokyo-night");
}

#[test]
fn terminals_without_truecolor_get_256_color_palettes() {
    let mut h = harness();
    h.app.truecolor = false;
    h.ctrl('t');
    h.press(KeyCode::Esc);
    assert!(
        matches!(h.app.theme.bg, ratatui::style::Color::Indexed(_)),
        "{:?}",
        h.app.theme.bg
    );
    assert!(matches!(
        h.screen_buffer()[(3, 5)].bg,
        ratatui::style::Color::Indexed(_)
    ));
}

fn open_new_notebook(h: &mut Harness) {
    h.create_notebook("N", "");
    h.press(KeyCode::Enter);
}

#[test]
fn model_picker_lists_every_ready_provider_and_stores_the_provider_with_the_model() {
    let mut h = harness();
    set_openrouter_url(
        &mut h,
        fake_openrouter(vec![
            r#"{"data":[{"id":"alpha/free","pricing":{"prompt":"0","completion":"0"}}]}"#.into(),
        ]),
    );
    let anthropic_url = fake_openrouter(vec![
        r#"{"data":[{"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","max_input_tokens":200000}]}"#.into(),
    ]);
    set_provider(&mut h, "anthropic", |p| {
        p.api_key = Some("sk-ant".into());
        p.base_url = Some(anthropic_url);
    });
    set_provider(&mut h, "claude-cli", |p| p.command = "sh".into());
    open_new_notebook(&mut h);

    h.ctrl('o');
    h.wait_for("picker", |a| a.modal.is_some());
    let s = h.screen_text();
    for want in [
        "alpha/free (free)",
        "claude-sonnet-5-5",
        "sonnet",
        "haiku",
        "openrouter",
        "anthropic",
        "claude-cli",
    ] {
        assert!(s.contains(want), "missing {want:?}\n{s}");
    }

    h.type_str("claude-sonnet");
    h.press(KeyCode::Enter);
    let nb = h.app.nb.as_ref().unwrap();
    assert_eq!(nb.model.as_deref(), Some("anthropic::claude-sonnet-5-5"));
    assert_eq!(
        h.app
            .store
            .get_notebook(nb.id)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("anthropic::claude-sonnet-5-5")
    );
    assert!(
        h.screen_text().contains("anthropic · claude-sonnet-5-5"),
        "{}",
        h.screen_text()
    );

    // An OpenRouter pick is stored bare, as before multi-provider support.
    h.ctrl('o');
    h.wait_for("cached picker", |a| a.modal.is_some());
    assert!(h.screen_text().contains('●'), "the current model is marked");
    h.type_str("alpha");
    h.press(KeyCode::Enter);
    let id = h.app.nb.as_ref().unwrap().id;
    assert_eq!(
        h.app
            .store
            .get_notebook(id)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("alpha/free")
    );
    assert!(h.screen_text().contains("alpha/free"));
}

#[test]
fn chat_goes_through_the_anthropic_api() {
    let mut h = harness();
    let url = fake_openrouter(vec![
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Claude says \"}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello.\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n".into(),
    ]);
    set_provider(&mut h, "anthropic", |p| {
        p.api_key = Some("sk-ant".into());
        p.base_url = Some(url);
    });
    open_new_notebook(&mut h);
    h.app.nb.as_mut().unwrap().model = Some("anthropic::claude-x".into());
    h.type_str("hi");
    h.press(KeyCode::Enter);
    h.wait_for("answer", |a| !a.nb.as_ref().unwrap().generating);
    let s = h.screen_text();
    assert!(s.contains("AI  ◄ Claude says hello."), "{s}");
    let last = h.app.nb.as_ref().unwrap().messages.last().unwrap().clone();
    assert_eq!(
        (last.role.as_str(), last.model.as_deref()),
        ("assistant", Some("anthropic::claude-x"))
    );
}

#[test]
fn chat_goes_through_the_claude_plan_cli_using_the_default_model() {
    let mut h = harness();
    let script = h._dir.path().join("fake_claude.sh");
    std::fs::write(
        &script,
        r#"cat > /dev/null
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"From the plan"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"message_stop"}}'
printf '%s\n' '{"type":"result","is_error":false,"result":"From the plan"}'
"#,
    )
    .unwrap();
    let command = format!("sh {}", script.display());
    set_provider(&mut h, "claude-cli", |p| p.command = command);
    h.app.cfg.default_model = Some("claude-cli::sonnet".into());
    open_new_notebook(&mut h);

    h.type_str("hello");
    h.press(KeyCode::Enter);
    h.wait_for("answer", |a| !a.nb.as_ref().unwrap().generating);
    let s = h.screen_text();
    assert!(
        s.contains("AI  ◄ From the plan") && s.contains("claude-cli · sonnet"),
        "{s}"
    );
    assert_eq!(
        h.app
            .nb
            .as_ref()
            .unwrap()
            .messages
            .last()
            .unwrap()
            .model
            .as_deref(),
        Some("claude-cli::sonnet")
    );
}

#[test]
fn a_model_from_a_provider_without_a_key_explains_what_to_do() {
    let mut h = harness();
    open_new_notebook(&mut h);
    h.app.nb.as_mut().unwrap().model = Some("anthropic::claude-x".into());
    h.type_str("hi");
    h.press(KeyCode::Enter);
    h.wait_for("error", |a| {
        a.nb.as_ref()
            .unwrap()
            .messages
            .iter()
            .any(|m| m.role == "error")
    });
    assert!(
        h.screen_text().contains("Anthropic API needs an API key"),
        "{}",
        h.screen_text()
    );
}

fn add_assistant_message(h: &mut Harness, content: &str) {
    let nb = h.app.nb.as_mut().unwrap();
    let id = nb.id;
    let saved = h
        .app
        .store
        .add_message(&rsnotebook::models::Message::new(id, "assistant", content))
        .unwrap();
    h.app.nb.as_mut().unwrap().messages.push(saved);
}

const MD_REPLY: &str = "## Summary\n\nThe **launch** is on *March 3rd*, see `facts.md`.\n\n- first point\n- second point\n\n```rust\nfn main() {}\n```\n\n| Item | Cost |\n|---|--:|\n| Aurora | 40k |";

#[test]
fn assistant_replies_are_rendered_as_markdown_and_user_text_is_not() {
    let mut h = harness();
    open_new_notebook(&mut h);
    let id = h.app.nb.as_ref().unwrap().id;
    let user = h
        .app
        .store
        .add_message(&rsnotebook::models::Message::new(
            id,
            "user",
            "is **this** markdown?",
        ))
        .unwrap();
    h.app.nb.as_mut().unwrap().messages.push(user);
    add_assistant_message(&mut h, MD_REPLY);

    let s = h.screen_text();
    assert!(
        s.contains("You ► is **this** markdown?"),
        "your own text is shown as typed\n{s}"
    );
    for want in [
        "Summary",
        "The launch is on March 3rd, see facts.md.",
        "• first point",
        "• second point",
        "┤ rust ├",
        "│ fn main() {}",
        "│ Aurora │",
    ] {
        assert!(s.contains(want), "missing {want:?}\n{s}");
    }
    for raw in [
        "## Summary",
        "**launch**",
        "*March 3rd*",
        "`facts.md`",
        "```",
        "|---|",
    ] {
        assert!(!s.contains(raw), "raw markdown leaked: {raw:?}\n{s}");
    }

    // The same reply, styled: bold really is bold and the heading uses the accent color.
    let buf = h.screen_buffer();
    let find = |needle: &str| {
        (0..buf.area.height).find_map(|y| {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            row.find(needle).map(|x| (x as u16, y))
        })
    };
    let (x, y) = find("launch").expect("bold word on screen");
    assert!(
        buf[(x, y)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    let (x, y) = find("Summary").expect("heading on screen");
    assert_eq!(
        buf[(x, y)].fg,
        h.app.theme.accent2,
        "h2 uses the secondary accent"
    );
}

#[test]
fn f2_toggles_markdown_and_remembers_it() {
    let mut h = harness();
    open_new_notebook(&mut h);
    add_assistant_message(&mut h, "plain **bold** text");
    assert!(h.screen_text().contains("plain bold text"));

    h.press(KeyCode::F(2));
    assert!(!h.app.markdown);
    let s = h.screen_text();
    assert!(
        s.contains("plain **bold** text") && s.contains("Markdown rendering off"),
        "{s}"
    );
    let saved = std::fs::read_to_string(h.app.cfg.dir().join("config.toml")).unwrap();
    assert!(saved.contains("markdown = false"), "{saved}");

    h.press(KeyCode::F(2));
    assert!(h.app.markdown && h.screen_text().contains("plain bold text"));

    // A fresh app reads the saved choice.
    h.press(KeyCode::F(2));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let again = App::new(
        Store::open_in_memory().unwrap(),
        Config::load(Some(h.app.cfg.dir().join("config.toml"))),
        rt.handle().clone(),
    );
    assert!(!again.markdown);
}

#[test]
fn the_rendered_history_follows_width_theme_and_new_messages() {
    let mut h = harness();
    open_new_notebook(&mut h);
    add_assistant_message(&mut h, "# One");
    assert!(h.screen_text().contains("One"));
    // Appending a message shows up even though history is cached.
    add_assistant_message(&mut h, "**Two**");
    let s = h.screen_text();
    assert!(s.contains("One") && s.contains("Two") && !s.contains("**Two**"));
    // Changing the theme re-renders with the new colors.
    let before = h.screen_buffer();
    h.ctrl('t');
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    let after = h.screen_buffer();
    assert_ne!(before[(40, 3)].bg, after[(40, 3)].bg);
    assert!(h.screen_text().contains("Two"));
}

#[test]
fn a_streaming_reply_is_rendered_as_it_arrives() {
    let mut h = harness();
    open_new_notebook(&mut h);
    {
        let nb = h.app.nb.as_mut().unwrap();
        nb.generating = true;
        nb.live = "# Plan\n\n- step **one**\n- step tw".into();
    }
    let s = h.screen_text();
    assert!(
        s.contains("Plan") && s.contains("• step one") && s.contains("• step tw"),
        "{s}"
    );
    assert!(!s.contains("**one**") && !s.contains("# Plan"));

    // An unfinished code fence and half-typed emphasis never break the screen.
    h.app.nb.as_mut().unwrap().live = "Look:\n\n```py\nprint('x'".into();
    let s = h.screen_text();
    assert!(s.contains("┤ py ├") && s.contains("print('x'"), "{s}");
    h.app.nb.as_mut().unwrap().live = "half **bol".into();
    assert!(h.screen_text().contains("half **bol"));
}

#[test]
fn hostile_markdown_cannot_inject_escapes_or_break_layout() {
    let mut h = harness();
    open_new_notebook(&mut h);
    add_assistant_message(
        &mut h,
        "evil &#27;[2J \u{1b}[31mred [x](http://a/\u{1b}]8;;b) \n\n```\u{1b}[1mx\n\u{1b}[2J\n```\n\n| \u{1b}[2J | b |\n|--|--|\n| &#27;x | y |",
    );
    let s = h.screen_text();
    assert!(!s.contains('\u{1b}'), "{s}");
    assert!(s.contains("evil") && s.contains("red"));
    for line in s.lines() {
        assert_eq!(line.chars().count(), 110, "layout intact: {line:?}");
    }
}

#[test]
fn a_streamed_chat_answer_ends_up_rendered() {
    let mut h = harness();
    set_openrouter_url(
        &mut h,
        fake_openrouter(vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"The **sky** is\\n\\n- blue\\n- vast\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into(),
        ]),
    );
    h.app.cfg.default_model = Some("m/x".into());
    open_new_notebook(&mut h);
    h.type_str("colour?");
    h.press(KeyCode::Enter);
    h.wait_for("answer", |a| !a.nb.as_ref().unwrap().generating);
    let s = h.screen_text();
    assert!(
        s.contains("The sky is")
            && s.contains("• blue")
            && s.contains("• vast")
            && !s.contains("**sky**"),
        "{s}"
    );
    // What is stored (and sent back as history) is the original Markdown source.
    let stored = h
        .app
        .store
        .list_messages(h.app.nb.as_ref().unwrap().id, None)
        .unwrap();
    assert_eq!(
        stored.last().unwrap().content,
        "The **sky** is\n\n- blue\n- vast"
    );
}
