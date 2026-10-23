# rsnotebook

A minimal terminal NotebookLM-style app, written in Rust with [ratatui](https://ratatui.rs).
Attach local files as sources, then chat with them using OpenRouter, the Anthropic API, your Claude plan,
or any OpenAI-compatible endpoint. Everything is stored locally in SQLite.

The interface uses extended-ASCII box drawing. Panes, table columns and dividers are joined with
real junctions (`┌ ┬ ┐ ├ ┼ ┤ └ ┴ ┘`), dialogs use double lines (`╔═╗`) with a shadow, scrollbars use
`░█`, and the context budget is shown as a meter. Everything is themeable (see below).

## Requirements

- Rust 1.88+ (edition 2024); no system libraries needed (SQLite is bundled, TLS is rustls)
- [ranger](https://github.com/ranger/ranger) for the file picker (optional; a path prompt is the fallback)

## Build and run

```bash
make run            # optimised build, then start the app
make install        # install the `rsnotebook` binary into ~/.cargo/bin
make help           # list every target
```

Without `make`: `cargo run --release`, or `cargo build --release` and run `./target/release/rsnotebook`.

On first launch the app opens the **providers** dialog (`Ctrl+K`, also available any time). Pick a
provider, paste its key, and you're set; keys are saved to `~/.config/rsnotebook/config.toml` with mode
`0600`. Keys can also come from the environment (`OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`) and are never
written back to disk.

- Config: `~/.config/rsnotebook/config.toml` (`default_model`, `theme`, `markdown`, `ranger_command`, `context_token_budget`, `system_prompt`, `[providers.*]`)
- Data: `~/.local/share/rsnotebook/rsnotebook.db`

`$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` are honoured. The app used to be called `pynotebook`: if only the old
`pynotebook` folders exist they are used as they are, so existing settings and notebooks keep working. To
switch to the new names, move `~/.config/pynotebook` to `~/.config/rsnotebook`, and
`~/.local/share/pynotebook` to `~/.local/share/rsnotebook` (renaming `pynotebook.db` to `rsnotebook.db`).

## Keybindings

### Notebook list
| Key | Action |
| --- | --- |
| `n` / `e` / `d` | new / edit / delete notebook (delete asks first) |
| `Enter` | open notebook |
| `/` | filter notebooks (`Esc` clears) |
| `m` | choose the default model |
| `t` | choose a color theme |
| `Ctrl+K` | providers and API keys |
| `F1` or `?` | help |
| `q` | quit |

### Inside a notebook
| Key | Action |
| --- | --- |
| `Tab` | switch focus between sources and chat |
| `Ctrl+O` | choose a model for this notebook (`Ctrl+D` in the picker also saves it as the default) |
| `Ctrl+L` | clear chat (asks first) |
| `PgUp` / `PgDn` | scroll the chat |
| `F2` | turn Markdown rendering of replies on/off (remembered) |
| `Esc` | cancel generation, otherwise back to the list |

Sources (when focused): `a` add (ranger, falls back to a path/glob prompt), `Space` toggle,
`x` remove, `r` re-import from disk, `Enter` preview.

Chat (when focused): `Enter` send, `Ctrl+J` / `Alt+Enter` newline.

`Ctrl+T` (color theme) and `Ctrl+K` (providers) work everywhere, and `Ctrl+C` quits.

## Providers

| Provider | Needs | Models |
| --- | --- | --- |
| **OpenRouter** | API key (`OPENROUTER_API_KEY`) | the full OpenRouter catalog, with prices |
| **Anthropic API** | API key (`ANTHROPIC_API_KEY`), pay-as-you-go | from Anthropic's model list |
| **Claude plan** | the `claude` CLI, logged in with your Pro/Max plan | `default`, `sonnet`, `opus`, `haiku` |
| **Custom** | any OpenAI-compatible endpoint (OpenAI, Ollama, LM Studio, vLLM, ...) | from its `/models` |

`Ctrl+O` in a notebook (or `m` on the list) opens one picker with the models of **every ready provider**,
searchable by provider, id or name. The choice is stored per notebook; `Ctrl+D` in the picker also makes it the
default. A model is saved as `provider::model` (for example `anthropic::claude-sonnet-5-5`); OpenRouter models
stay bare ids, exactly as before multi-provider support, so older notebooks keep working.

### Using your Claude plan

rsnotebook does not read or copy your Claude login token. It runs the official `claude` CLI headlessly
(`claude -p --output-format stream-json`), so your plan is used by Claude Code itself:

1. Install Claude Code and run `claude` once to sign in with your plan.
2. Pick a `claude-cli` model in the model picker.

Details worth knowing:

- All tools, MCP servers and slash commands are disabled, and the CLI runs in an empty temporary directory.
  Your sources are untrusted text; this stops anything injected in them from acting on your machine.
- `ANTHROPIC_API_KEY` is removed from the CLI's environment, so it can never bill the API instead of your plan.
- Answers count against your plan's usage limits. Because the CLI is a single prompt in, text out, chat history
  is passed as a transcript.
- The model picker's aliases (`sonnet`, `opus`, `haiku`) always point at the latest model of that family.

### Custom and OpenAI-compatible endpoints

Add a table to the config file. `kind` defaults to `openai-compatible`; the key is optional (local servers).

```toml
[providers.ollama]
name = "Ollama"
base_url = "http://localhost:11434/v1"

[providers.work]
name = "Work gateway"
base_url = "https://llm.example.com/v1"
api_key_env = "WORK_LLM_KEY"      # read from the environment; or set api_key = "..." to store it

[providers.claude-cli]            # optional: change how the CLI is launched
command = "claude"
```

Provider ids may use letters, digits, `-` and `_`.

## Themes

`Ctrl+T` (or `t` on the notebook list) opens the theme picker. Moving the cursor previews a theme
live; `Enter` applies it and saves it to the config (`theme = "nord"`), `Esc` reverts.

Built in: Tokyo Night (default), Dracula, Nord, Gruvbox Dark, Catppuccin Mocha, One Dark,
Solarized Dark, Solarized Light, Catppuccin Latte, Gruvbox Light, and *Terminal colors*, which
uses your terminal's own background and 16 ANSI colors.

Truecolor is used when `COLORTERM` is `truecolor`/`24bit`; otherwise colors are mapped to the nearest
256-color palette entry.

### Custom themes

Drop a `.toml` file in `~/.config/rsnotebook/themes/`. Every key is optional; missing colors come from
`base` (a built-in id, default `tokyo-night`). A file whose `id` matches a built-in replaces it.

```toml
# ~/.config/rsnotebook/themes/neon.toml
name = "Neon"          # display name (default: the file name)
base = "dracula"       # start from this theme
light = false
bg = "#0b0c10"         # background
fg = "#c5c6c7"         # text
dim = "#6b7280"        # hints, placeholders
accent = "#00ffcc"     # focused borders, header bar, selection pointer
accent2 = "#ff6ac1"    # model name, secondary highlights
border = "#2a2d34"     # unfocused borders
sel_bg = "#1f2229"     # selected row, status/hint strips
bar_fg = "#0b0c10"     # text on the accent-colored header bar
green = "#66fcf1"      # assistant, success
red = "#ff5370"        # errors
yellow = "#ffcb6b"     # warnings
cyan = "#89ddff"       # your messages
```

Invalid files are skipped and reported on startup.

## Markdown in replies

The assistant's replies are rendered as Markdown, styled with the current theme and wrapped to the pane:

- headings (`#`, `##` get an underline rule), **bold**, *italic*, ~~strikethrough~~, `inline code`
- bulleted, numbered and nested lists, task lists (`[x]`), block quotes (`│`), horizontal rules
- fenced code blocks drawn as a box with the language in the title, long lines wrapped inside the frame
- tables drawn with the same linked box-drawing glyphs as the rest of the UI, honouring `:--` / `:-:` / `--:`
  alignment and wrapping cells when the pane is narrow
- links shown as `text (url)`; images as `[image: alt]`. Links are not clickable, so nothing can open or
  hide a destination from you

It works while a reply streams in too, so a half-finished list or code block already looks right. Your own
messages and error messages are shown exactly as typed. What is saved and sent back to the model as history is
always the original Markdown source. Everything shown has terminal control characters removed *after* parsing,
because Markdown entities like `&#27;` can decode to them. `F2` switches to raw text and back; the choice is
saved as `markdown = true|false` in the config.

## How it works

- Sources (text/code, PDF, DOCX, HTML) are extracted, stored, and split into chunks indexed by SQLite FTS5.
- If all enabled sources fit the context budget their full text is sent; otherwise the best BM25 chunks for
  the question are sent. Older chat turns are trimmed to fit.
- Answers stream (SSE, or the CLI's JSON stream). `Esc` cancels and keeps the partial answer.

## Good to know

- `Ctrl+O` picks the model: terminals send `Ctrl+M` as `Enter`, so it cannot be bound.
- If the database cannot be opened the app prints the error and exits instead of starting empty.
- Models from non-OpenRouter providers are stored as `provider::model`; OpenRouter models stay bare ids.

## Safety notes

- Text from files and from the model is stripped of terminal control characters before drawing.
- Source text cannot forge the `=== SOURCE ... ===` delimiters used in the prompt; the prompt also tells the
  model to treat source text as data. This reduces, but cannot eliminate, prompt injection.
- The Claude-plan provider runs with every tool disabled (see above) so prompt injection in a source cannot execute anything.
- Files are capped at 20 MB and extracted text at 5M characters; DOCX/PDF limits guard against zip/page bombs.
- HTTP redirects are never followed, so an API key cannot be forwarded elsewhere; keys never appear in `Debug` output.

## Development

```bash
make check          # what CI would run: rustfmt --check, clippy -D warnings, all tests
make test           # tests only
make fmt            # format
make dev            # run a debug build (compiles faster)
```

Layout:

| Path | Role |
| --- | --- |
| `src/main.rs`, `src/app.rs` | entry point; app state, key handling, background results, terminal loop |
| `src/chat.rs` | one chat turn: build the prompt, stream the answer, persist the outcome |
| `src/store.rs`, `src/models.rs` | SQLite persistence (FTS5 chunk index) and the shared data types |
| `src/config.rs` | settings, providers, API keys, file locations |
| `src/sources/` | text extraction (text, PDF, DOCX, HTML), chunking, importing, the ranger picker |
| `src/llm/` | providers (`provider.rs` dispatches), SSE parsing, model cache, prompt/context budgeting |
| `src/ui/` | widgets and drawing: screens, modals, themes, box-drawing frames, Markdown, text input |
| `tests/` | integration tests: the whole app driven by key presses against fake servers |

The logic and I/O modules (`store`, `sources`, `llm`, `chat`, `config`) contain no terminal code, so they are
tested without a terminal. `tests/app_flow.rs` drives the app through key presses against an in-memory terminal
and fake OpenRouter / Anthropic / `claude` backends.
