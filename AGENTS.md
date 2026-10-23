# AGENTS.md

## Conventions
- Keep changes consistent with the structure, naming, style, and patterns of the surrounding code.
- Treat unfamiliar files or changes as potential user work and investigate before deleting or overwriting them.
- UI, domain logic and I/O are separated so the logic is testable without the terminal:
  `store`, `sources`, `llm`, `chat` and `config` have no terminal code; `ui` and `app` are the only parts that draw or read keys.
- Never log or print an API key. Types that hold keys (`ProviderConfig`, the HTTP clients) implement `Debug` without them; keep it that way.
- Don't leave TODO stubs: every listed feature must work end to end.

## How to run/test
- Run app: `make run` (optimised) or `make dev` (debug)
- Test: `make test`
- Lint + format check + tests (what CI would run): `make check`
- Format: `make fmt`
- `make help` lists every target; without `make`, use the equivalent `cargo` commands.

## Layout
- `src/main.rs` entry point; `src/app.rs` state, key handling, background results and the terminal loop
- `src/chat.rs` one chat turn (prompt -> stream -> persist)
- `src/store.rs`, `src/models.rs` SQLite (FTS5) and shared types
- `src/config.rs` settings, providers, keys, file locations
- `src/sources/` extraction, chunking, import, ranger picker
- `src/llm/` providers (`provider.rs` dispatches), SSE, model cache, context budgeting
- `src/ui/` screens, modals, themes, box-drawing frames, Markdown rendering, text input
- `tests/` integration tests (the app driven by key presses against fake backends)

## Rules that must keep holding
- Text from files, models and names is untrusted: it goes through `ui::text::safe_text` (and, for Markdown, is
  cleaned after parsing) before it is drawn.
- Keys from the environment are never written to disk; only keys the user saved are.
- The Claude-plan provider (`llm/claude_cli.rs`) keeps every CLI tool disabled, runs in a throwaway directory and
  strips `ANTHROPIC_API_KEY` from the child's environment.
- Database access goes through the single mutex-guarded connection in `store.rs`; schema changes are new
  `PRAGMA user_version` migrations.
- Config and data live in `rsnotebook` folders; the legacy `pynotebook` folders are still used when they are the only ones present.
