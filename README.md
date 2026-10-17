# pynotebook

A minimal terminal-based NotebookLM-style app built with [Textual](https://textual.textualize.io/).

## Requirements

- Python 3.11+
- [uv](https://github.com/astral-sh/uv) for dependency management
- [ranger](https://github.com/ranger/ranger) for file picker (optional — fallback path input exists)

## Setup

```bash
uv sync
```

Set your OpenRouter API key:

```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

If not set, the app will prompt you on first launch to enter it. It will be saved to `~/.config/pynotebook/config.toml` with mode `0600`.

## Usage

```bash
uv run pynotebook
```

Data and config follow XDG conventions:
- Config: `~/.config/pynotebook/config.toml`
- Data: `~/.local/share/pynotebook/pynotebook.db`

## Keybindings

### Notebook list
- `n` new notebook
- `e` edit notebook
- `d` delete notebook (confirm)
- `enter` open notebook
- `/` filter notebooks
- `m` choose default model
- `q` quit

### Notebook
#### Sources
- `a` add source (ranger; fallback to manual path input)
- `space` toggle enabled
- `x` remove source (confirm)
- `r` re-import from disk
- `enter` preview text

#### Chat
- `Enter` send message
- `Shift+Enter` / `Ctrl+J` newline
- `Ctrl+M` model picker
- `Ctrl+L` clear chat (confirm)
- `Esc` cancel generation / back to notebook list
- `?` this help

## Features

- Create, edit, delete, open notebooks
- Attach local files via ranger (text, PDF, DOCX, HTML, plain text/code/json/csv etc.)
- Toggle sources on/off; remove or re-import
- Chat grounded in enabled sources with streaming responses
- Per-notebook model with OpenRouter; model picker and global default
- Context budget with full-text or BM25 retrieval fallback
- SSE streaming with graceful error handling and cancellation
- Persists to SQLite with FTS5 for retrieval

## Limitations

This is a deliberately minimal take on NotebookLM-style apps. Out of scope: web UI, API server, user accounts, URLs/YouTube/audio/video sources, embeddings/vector DB, podcasts, notes/transformations, multiple providers other than OpenRouter, plugins, sync.

## Development

```bash
uv run ruff check
uv run pytest
```
