from __future__ import annotations

from pathlib import Path
from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Markdown, Static, TextArea

from ... import models
from ...sources import process, ranger
from ...sources.extract import estimate_tokens


class NotebookScreen(Screen):
    BINDINGS: ClassVar[list[Binding]] = [
        Binding("a", "add_source", "Add source"),
        Binding("space", "toggle_source", "Toggle"),
        Binding("x", "remove_source", "Remove"),
        Binding("r", "reimport_source", "Re-import"),
        Binding("enter", "preview_source", "Preview"),
        Binding("ctrl+m", "model_picker", "Model"),
        Binding("ctrl+l", "clear_chat", "Clear chat"),
        Binding("escape", "back", "Back", show=True),
        Binding("question_mark", "help", "Help"),
    ]

    def __init__(self, notebook_id: int, notebook_name: str) -> None:
        super().__init__()
        self.notebook_id = notebook_id
        self.notebook_name = notebook_name
        self._sources: list[models.Source] = []
        self._model: str | None = None

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal():
            with Vertical(id="sources-pane"):
                yield Static("Sources", id="sources-title")
                yield DataTable(id="sources")
            with Vertical(id="chat-pane"):
                yield Markdown("", id="chat-log")
                yield TextArea(id="chat-input")
                yield Static("", id="chat-status")
        yield Footer()

    async def on_mount(self) -> None:
        self.title = self.notebook_name
        table = self.query_one("#sources", DataTable)
        table.add_columns("On", "Filename", "Status", "Tokens")
        if self.repo:
            nb = await self.repo.get_notebook(self.notebook_id)
            if nb:
                self._model = nb.model
        await self.refresh_sources()

    @property
    def repo(self):
        return self.app.repo  # type: ignore[attr-defined]

    def cfg(self):
        return self.app.cfg  # type: ignore[attr-defined]

    async def refresh_sources(self) -> None:
        if not self.repo:
            return
        self._sources = await self.repo.list_sources(self.notebook_id)
        table = self.query_one("#sources", DataTable)
        table.clear()
        for s in self._sources:
            on = "[x]" if s.enabled else "[ ]"
            table.add_row(on, s.filename, s.status, str(s.token_estimate), key=str(s.id))
        self._update_footer()

    def _update_footer(self) -> None:
        total = sum(s.token_estimate for s in self._sources if s.enabled)
        count = sum(1 for s in self._sources if s.enabled)
        budget = self.cfg().context_token_budget
        status = self.query_one("#chat-status", Static)
        model = self._model or self.cfg().default_model or "(no model)"
        status.update(f"{model} | {total / 1000:.1f}k / {budget // 1000}k tokens, {count} sources")

    def _cursor_source(self) -> models.Source | None:
        table = self.query_one("#sources", DataTable)
        if table.cursor_row is None:
            return None
        if table.cursor_row >= len(self._sources):
            return None
        return self._sources[table.cursor_row]

    async def action_add_source(self) -> None:
        # Run ranger inside suspend to hand over terminal
        paths: list[Path] = []
        ranger_cmd = self.cfg().ranger_command
        try:
            with self.app.suspend():
                paths = ranger.pick_files(ranger_cmd)
        except Exception:  # noqa: BLE001
            paths = []

        if not paths:
            # Fallback: path input dialog
            from ..modals import PathInputDialog

            def _on_path(value: str | None) -> None:
                if value:
                    self.run_worker(self._add_paths_from_input(value), exclusive=True)

            self.app.push_screen(
                PathInputDialog("Ranger unavailable or no selection. Enter a file path (or glob):"),
                callback=_on_path,
            )
            return

        await self._add_paths(paths)

    async def _add_paths_from_input(self, value: str) -> None:
        from glob import glob

        value = value.strip()
        expanded = str(Path(value).expanduser())
        matches = sorted(glob(expanded))
        paths = [Path(m) for m in matches if Path(m).is_file()]
        if not paths:
            p = Path(expanded)
            if p.is_file():
                paths = [p]
        if not paths:
            self.app.notify(f"No files matched: {value}", severity="warning")
            return
        await self._add_paths(paths)

    async def _add_paths(self, paths: list[Path]) -> None:
        if not self.repo:
            return
        added = 0
        errors = 0
        for p in paths:
            if not p.is_file():
                continue
            source, chunks = process.build_source_from_path(self.notebook_id, p)
            source = await self.repo.add_source(source)
            if source.id is not None and chunks:
                await self.repo.add_chunks(source.id, list(enumerate(chunks)))
            if source.status == "error":
                errors += 1
            else:
                added += 1
        await self.refresh_sources()
        if errors and not added:
            self.app.notify(f"Could not import {errors} file(s)", severity="error")
        elif errors:
            self.app.notify(f"Added {added} source(s), {errors} failed", severity="warning")
        else:
            self.app.notify(f"Added {added} source(s)")

    async def action_toggle_source(self) -> None:
        source = self._cursor_source()
        if not source or not self.repo or source.id is None:
            return
        await self.repo.toggle_source(source.id, not source.enabled)
        await self.refresh_sources()

    async def action_remove_source(self) -> None:
        source = self._cursor_source()
        if not source or not self.repo or source.id is None:
            return
        from ..modals import ConfirmDialog

        def _on_confirm(confirmed: bool) -> None:
            if confirmed:
                self.run_worker(self._remove_source(source.id or 0), exclusive=True)

        self.app.push_screen(ConfirmDialog(f"Remove source '{source.filename}'?"), callback=_on_confirm)

    async def _remove_source(self, source_id: int) -> None:
        if not self.repo:
            return
        await self.repo.delete_source(source_id)
        await self.refresh_sources()

    async def action_reimport_source(self) -> None:
        source = self._cursor_source()
        if not source or not self.repo or source.id is None:
            return
        self.run_worker(self._reimport(source), exclusive=True)

    async def _reimport(self, source: models.Source) -> None:
        if not self.repo or source.id is None:
            return
        from ...sources import extract

        p = Path(source.path)
        if not p.is_file():
            await self.repo.update_source_content(
                source.id, source.content_text, source.token_estimate, status="error", error="File no longer exists"
            )
            await self.refresh_sources()
            self.app.notify(f"File missing: {source.path}", severity="error")
            return
        content, _size, _ext, _mime, err = extract.extract_text(p)
        token_estimate = estimate_tokens(content)
        await self.repo.update_source_content(
            source.id,
            content,
            token_estimate,
            status="ok" if err is None else "error",
            error=err,
        )
        await self.repo.clear_chunks(source.id)
        if err is None and content:
            from ...sources import chunk

            chunks = chunk.chunk_text(content)
            await self.repo.add_chunks(source.id, list(enumerate(chunks)))
        await self.refresh_sources()

    async def action_preview_source(self) -> None:
        source = self._cursor_source()
        if not source:
            return
        from ..modals import SourcePreview

        text = source.content_text or source.error or "(no content)"
        self.app.push_screen(SourcePreview(source.filename, text))

    async def action_model_picker(self) -> None:
        from ..modals import ModelPicker

        models = await self.app.get_models()  # type: ignore[attr-defined]
        if not models:
            self.app.notify("No models available. Check your API key.", severity="warning")
            return
        nb = await self.repo.get_notebook(self.notebook_id) if self.repo else None
        current = (nb.model if nb else None) or self.cfg().default_model

        def _on_dismiss(selection: object) -> None:
            if selection is None:
                return
            model_id = getattr(selection, "model_id", None)
            save_default = getattr(selection, "save_as_default", False)
            if not model_id:
                return
            self.run_worker(self._apply_model(model_id, save_default), exclusive=True)

        self.app.push_screen(ModelPicker(models, current=current), callback=_on_dismiss)

    async def _apply_model(self, model_id: str, save_default: bool) -> None:
        self._model = model_id
        if self.repo:
            await self.repo.update_notebook(self.notebook_id, model=model_id)
        if save_default:
            cfg = self.cfg()
            cfg.default_model = model_id
            from ... import config

            config.save_config(cfg)
        # Clamp budget if model context is smaller
        models = await self.app.get_models()  # type: ignore[attr-defined]
        info = next((m for m in models if m.id == model_id), None)
        if info and info.context_length:
            cfg = self.cfg()
            available = max(info.context_length - 4000, 1000)
            if available < cfg.context_token_budget:
                cfg.context_token_budget = available
                self.app.notify(
                    f"Context budget clamped to {available} tokens for {model_id}"
                )
        self._update_footer()

    async def action_clear_chat(self) -> None:
        pass

    async def action_help(self) -> None:
        pass

    async def action_back(self) -> None:
        await self.app.pop_screen()
