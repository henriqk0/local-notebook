# AGENTS.md

## Conventions
- Keep changes consistent with the structure, naming, style, and patterns of the surrounding code.
- Treat unfamiliar files or changes as potential user work and investigate before deleting or overwriting them.
- UI, domain logic and I/O separated so the logic is testable without the TUI.
- Never log or print the API key.
- Don't leave TODO stubs: every listed feature must work end to end.

## How to run/test
- Install deps: `uv sync`
- Run app: `uv run pynotebook`
- Lint: `ruff check`
- Format: `ruff format` (if needed)
- Test: `pytest`

## Phase tracking
Phase 1: Scaffold project (uv, deps, entry point, AGENTS.md, empty app that launches) - IN PROGRESS
Phase 2: Config + SQLite layer + repository with tests
Phase 3: Notebook list screen with full CRUD
Phase 4: Source extraction/chunking/FTS + ranger integration + sources panel
Phase 5: OpenRouter client + model cache + model picker
Phase 6: Context builder (full-text vs BM25 branches, budget trimming) with unit tests
Phase 7: Chat UI with streaming, cancel, persistence, error handling
Phase 8: Polish: help overlay, empty states, README with install/usage/keybindings, final test pass

## Assumptions made
- Using `aiosqlite` instead of stdlib `sqlite3` for async DB operations (compatible with Textual async patterns)
