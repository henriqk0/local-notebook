from __future__ import annotations

import asyncio
from pathlib import Path
from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import Footer, Header, Markdown, Static

from ... import models
from ...llm import context as context_mod
from ...llm import openrouter
from ...sources import process, ranger
from ...sources.extract import estimate_tokens
from ..widgets.chat import ChatInput
from ..widgets.sources_table import SourcesTable


class NotebookScreen(Screen):
    BINDINGS: ClassVar[list[Binding]] = [
        Binding("ctrl+m", "model_picker", "Model"),
        Binding("ctrl+l", "clear_chat", "Clear chat"),
        Binding("escape", "escape", "Back / Cancel", show=True),
        Binding("question_mark", "help", "Help"),
    ]

    def __init__(self, notebook_id: int, notebook_name: str) -> None:
        super().__init__()
        self.notebook_id = notebook_id
        self.notebook_name = notebook_name
        self._sources: list[models.Source] = []
        self._model: str | None = None
        self._messages: list[models.Message] = []
        self._gen_worker = None

    # --- composition / mount -------------------------------------------------

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal():
            with Vertical(id="sources-pane"):
                yield Static("Sources", id="sources-title")
                yield Static(
                    "No sources yet. Press 'a' to add files with ranger.",
                    id="sources-empty",
                )
                yield SourcesTable(id="sources", cursor_type="row")
            with Vertical(id="chat-pane"):
                with VerticalScroll(id="chat-log"):
                    pass
                yield ChatInput(id="chat-input")
                yield Static("", id="chat-status")
        yield Footer()

    async def on_mount(self) -> None:
        self.title = self.notebook_name
        table = self.query_one("#sources", SourcesTable)
        table.add_columns("On", "Filename", "Status", "Tokens")
        if self.repo:
            nb = await self.repo.get_notebook(self.notebook_id)
            if nb:
                self._model = nb.model
            self._messages = await self.repo.list_messages(self.notebook_id)
        await self.refresh_sources()
        await self._render_history()

    @property
    def repo(self):
        return self.app.repo  # type: ignore[attr-defined]

    def cfg(self):
        return self.app.cfg  # type: ignore[attr-defined]

    # --- sources -------------------------------------------------------------

    async def refresh_sources(self) -> None:
        if not self.repo:
            return
        self._sources = await self.repo.list_sources(self.notebook_id)
        table = self.query_one("#sources", SourcesTable)
        table.clear()
        for s in self._sources:
            on = "[x]" if s.enabled else "[ ]"
            table.add_row(on, s.filename, s.status, str(s.token_estimate), key=str(s.id))
        empty = self.query_one("#sources-empty", Static)
        empty.display = len(self._sources) == 0
        self._update_footer()

    def _update_footer(self) -> None:
        total = sum(s.token_estimate for s in self._sources if s.enabled)
        count = sum(1 for s in self._sources if s.enabled)
        budget = self.cfg().context_token_budget
        status = self.query_one("#chat-status", Static)
        model = self._model or self.cfg().default_model or "(no model)"
        status.update(f"{model} | {total / 1000:.1f}k / {budget // 1000}k tokens, {count} sources")

    def _cursor_source(self) -> models.Source | None:
        table = self.query_one("#sources", SourcesTable)
        if table.cursor_row is None or table.cursor_row >= len(self._sources):
            return None
        return self._sources[table.cursor_row]

    async def action_add_source(self) -> None:
        paths: list[Path] = []
        try:
            with self.app.suspend():
                paths = ranger.pick_files(self.cfg().ranger_command)
        except (OSError, RuntimeError):
            paths = []

        if not paths:
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
        from ...sources import chunk, extract

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
            source.id, content, token_estimate, status="ok" if err is None else "error", error=err
        )
        await self.repo.clear_chunks(source.id)
        if err is None and content:
            await self.repo.add_chunks(source.id, list(enumerate(chunk.chunk_text(content))))
        await self.refresh_sources()

    async def action_preview_source(self) -> None:
        source = self._cursor_source()
        if not source:
            return
        from ..modals import SourcePreview

        text = source.content_text or source.error or "(no content)"
        self.app.push_screen(SourcePreview(source.filename, text))

    # --- model ---------------------------------------------------------------

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
            self.run_worker(
                self._apply_model(
                    selection.model_id, selection.save_as_default  # type: ignore[attr-defined]
                ),
                exclusive=True,
            )

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
        models = await self.app.get_models()  # type: ignore[attr-defined]
        info = next((m for m in models if m.id == model_id), None)
        if info and info.context_length:
            cfg = self.cfg()
            available = max(info.context_length - 4000, 1000)
            if available < cfg.context_token_budget:
                cfg.context_token_budget = available
                self.app.notify(f"Context budget clamped to {available} tokens for {model_id}")
        self._update_footer()

    # --- chat ----------------------------------------------------------------

    async def _render_history(self) -> None:
        log = self.query_one("#chat-log", VerticalScroll)
        await log.remove_children()
        for msg in self._messages:
            await self._append_message(msg.role, msg.content, scroll=False)
        if self._messages:
            log.scroll_end(animate=False)

    async def _append_message(self, role: str, content: str, scroll: bool = True) -> Markdown:
        log = self.query_one("#chat-log", VerticalScroll)
        if role == "user":
            rendered = f"**You:** {content}"
        elif role == "error":
            rendered = f"> ⚠️ {content}"
        else:
            rendered = content
        md = Markdown(rendered, classes=f"msg-{role}")
        await log.mount(md)
        if scroll:
            log.scroll_end(animate=False)
        return md

    async def on_chat_input_submitted(self, event: ChatInput.Submitted) -> None:
        event.stop()
        await self._start_generation(event.text)

    # --- sources table messages ---------------------------------------------

    async def on_sources_table_add_source(self, event: SourcesTable.AddSource) -> None:
        event.stop()
        await self.action_add_source()

    async def on_sources_table_toggle_source(self, event: SourcesTable.ToggleSource) -> None:
        event.stop()
        await self.action_toggle_source()

    async def on_sources_table_remove_source(self, event: SourcesTable.RemoveSource) -> None:
        event.stop()
        await self.action_remove_source()

    async def on_sources_table_reimport_source(self, event: SourcesTable.ReimportSource) -> None:
        event.stop()
        await self.action_reimport_source()

    async def on_data_table_row_selected(self, event) -> None:
        if event.data_table.id == "sources":
            event.stop()
            await self.action_preview_source()

    async def _start_generation(self, question: str) -> None:
        if self._gen_worker is not None:
            return  # already generating
        if not self.repo:
            return

        prior_history = list(self._messages)
        user_msg = models.Message(notebook_id=self.notebook_id, role="user", content=question)
        user_msg = await self.repo.add_message(user_msg)
        self._messages.append(user_msg)
        await self._append_message("user", question)
        input_widget = self.query_one("#chat-input", ChatInput)
        input_widget.clear()

        self._gen_worker = self.run_worker(
            self._generate(question, prior_history), exclusive=True, name="chat"
        )

    async def _generate(self, question: str, prior_history: list[models.Message]) -> None:
        assert self.repo is not None
        model = self._model or self.cfg().default_model
        if not model:
            await self._finish_with_error("No model selected. Press Ctrl+M to choose one.")
            return
        if not self.cfg().api_key:
            await self._finish_with_error("No API key configured. Set OPENROUTER_API_KEY.")
            return

        sources = await self.repo.list_sources(self.notebook_id)
        match = context_mod.sanitize_fts_query(question)
        raw_hits = await self.repo.search_chunks(self.notebook_id, match) if match else []
        hits = [context_mod.ChunkHit(sid, fn, text) for sid, fn, text in raw_hits]

        ctx = context_mod.build_context(
            question,
            sources,
            prior_history,
            self.cfg().system_prompt,
            self.cfg().context_token_budget,
            search_chunks=lambda _q: hits,
        )

        messages: list[openrouter.ChatMessage] = []
        system_content = ctx.system_prompt
        if ctx.sources_block:
            system_content += "\n\n" + ctx.sources_block
        messages.append(openrouter.ChatMessage("system", system_content))
        for m in ctx.history:
            if m.role in ("user", "assistant"):
                messages.append(openrouter.ChatMessage(m.role, m.content))
        messages.append(openrouter.ChatMessage("user", question))

        assistant_md = await self._append_message("assistant", "")
        accumulated = ""
        client = openrouter.OpenRouterClient(self.cfg().api_key)
        try:
            async for token in client.stream_chat(model, messages):
                accumulated += token
                assistant_md.update(accumulated)
                log = self.query_one("#chat-log", VerticalScroll)
                log.scroll_end(animate=False)
        except asyncio.CancelledError:
            if accumulated:
                partial = accumulated + "\n\n_(interrupted)_"
                saved = await self.repo.add_message(
                    models.Message(
                        notebook_id=self.notebook_id,
                        role="assistant",
                        content=partial,
                        model=model,
                    )
                )
                self._messages.append(saved)
                assistant_md.update(partial)
            raise
        except openrouter.OpenRouterError as e:
            if accumulated:
                partial = accumulated + "\n\n_(interrupted: error)_"
                saved = await self.repo.add_message(
                    models.Message(
                        notebook_id=self.notebook_id,
                        role="assistant",
                        content=partial,
                        model=model,
                    )
                )
                self._messages.append(saved)
                assistant_md.update(partial)
            else:
                await self._record_error(e.message)
            await self._append_message("error", e.message)
        except Exception as e:  # noqa: BLE001
            await self._record_error(f"Unexpected error: {e}")
            await self._append_message("error", f"Unexpected error: {e}")
        else:
            await self.repo.add_message(
                models.Message(
                    notebook_id=self.notebook_id, role="assistant", content=accumulated, model=model
                )
            )
            self._messages.append(
                models.Message(notebook_id=self.notebook_id, role="assistant", content=accumulated, model=model)
            )
        finally:
            await client.aclose()
            self._gen_worker = None

    async def _finish_with_error(self, message: str) -> None:
        await self._append_message("error", message)
        await self._record_error(message)
        self._gen_worker = None

    async def _record_error(self, message: str) -> None:
        if not self.repo:
            return
        saved = await self.repo.add_message(
            models.Message(notebook_id=self.notebook_id, role="error", content=message)
        )
        self._messages.append(saved)

    # --- misc actions --------------------------------------------------------

    async def action_clear_chat(self) -> None:
        if not self.repo:
            return
        from ..modals import ConfirmDialog

        def _on_confirm(confirmed: bool) -> None:
            if confirmed:
                self.run_worker(self._clear_chat(), exclusive=True)

        self.app.push_screen(ConfirmDialog("Clear this notebook's chat history?"), callback=_on_confirm)

    async def _clear_chat(self) -> None:
        if not self.repo:
            return
        await self.repo.clear_messages(self.notebook_id)
        self._messages = []
        await self._render_history()

    async def action_help(self) -> None:
        from ..modals import SourcePreview

        focused = self.app.focused
        if isinstance(focused, ChatInput):
            focused.insert("?")
            return
        help_text = (
            "- Sources: `a` add, `space` toggle, `x` remove, `r` re-import, `enter` preview\n"
            "- Chat: type a message, `Enter` sends, `Shift+Enter`/`Ctrl+J` newline\n"
            "- `Ctrl+M` model picker, `Ctrl+L` clear chat\n"
            "- `Esc` cancel generation / back to notebook list\n"
            "- `?` this help"
        )
        self.app.push_screen(SourcePreview("Help", help_text))

    async def action_escape(self) -> None:
        if self._gen_worker is not None:
            self._gen_worker.cancel()
            self.app.notify("Generation cancelled")
            return
        await self.app.pop_screen()
