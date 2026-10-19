from __future__ import annotations

import asyncio
import re
import time
from contextlib import suppress
from pathlib import Path
from typing import ClassVar

from textual.app import ComposeResult, SuspendNotSupported
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import Footer, Header, Markdown, Static
from textual.worker import WorkerCancelled

from ... import models
from ...llm import context as context_mod
from ...llm import openrouter
from ...sources import chunk, extract, process, ranger
from ...tokens import estimate_tokens
from ..widgets.chat import ChatInput
from ..widgets.sources_table import SourcesTable

CHAT_GROUP = "chat"
IO_GROUP = "io"
MODEL_GROUP = "model"
UI_GROUP = "ui"

MAX_IMPORT_FILES = 100
MAX_PREVIEW_CHARS = 20_000
ANSWER_RESERVE_TOKENS = 1024

_CONTROL_CHARS = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]")


def _safe_text(text: str) -> str:
    """Strip terminal control characters from untrusted content."""
    return _CONTROL_CHARS.sub("", text)


def _format_tokens(count: int) -> str:
    if count >= 1_000_000:
        return f"{count / 1_000_000:.1f}M"
    if count >= 1_000:
        return f"{count / 1_000:.1f}k"
    return str(count)


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
        self._generating = False

    # --- composition / mount -------------------------------------------------

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal():
            with Vertical(id="sources-pane"):
                yield Static("Sources", id="sources-title")
                yield Static(
                    "No sources yet. Press Tab to focus sources, then 'a' to add files.",
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
            self._messages = await self.repo.list_messages(self.notebook_id, limit=200)
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
        self._sources = await self.repo.list_source_meta(self.notebook_id)
        table = self.query_one("#sources", SourcesTable)
        table.clear()
        for s in self._sources:
            on = "[x]" if s.enabled else "[ ]"
            table.add_row(
                on,
                s.filename,
                s.status,
                _format_tokens(s.token_estimate),
                key=str(s.id),
            )
        empty = self.query_one("#sources-empty", Static)
        empty.display = len(self._sources) == 0
        self._update_footer()

    def _update_footer(self) -> None:
        total = sum(s.token_estimate for s in self._sources if s.enabled)
        count = sum(1 for s in self._sources if s.enabled)
        budget = self.cfg().context_token_budget
        status = self.query_one("#chat-status", Static)
        model = self._model or self.cfg().default_model or "(no model)"
        generating = " | generating…" if self._generating else ""
        status.update(
            f"{model} | {_format_tokens(total)} / {_format_tokens(budget)} tokens, "
            f"{count} sources{generating}"
        )

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
        except (OSError, RuntimeError, SuspendNotSupported):
            paths = []

        if not paths:
            from ..modals import PathInputDialog

            def _on_path(value: str | None) -> None:
                if value:
                    self.run_worker(
                        self._add_paths_from_input(value),
                        group=IO_GROUP,
                        exclusive=True,
                        exit_on_error=False,
                    )

            self.app.push_screen(
                PathInputDialog(
                    "Ranger unavailable or no selection. Enter a file path (or glob):"
                ),
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

        unique: dict[str, Path] = {}
        for path in paths:
            if path.is_file():
                unique[str(path)] = path
        selected = list(unique.values())[:MAX_IMPORT_FILES]
        if len(unique) > MAX_IMPORT_FILES:
            self.app.notify(
                f"Only the first {MAX_IMPORT_FILES} files were imported.",
                severity="warning",
            )

        added = 0
        errors = 0
        for p in selected:
            try:
                source, chunks = await asyncio.to_thread(
                    process.build_source_from_path, self.notebook_id, p
                )
                await self.repo.add_source_with_chunks(source, list(enumerate(chunks)))
            except Exception:  # noqa: BLE001 - keep importing other files
                errors += 1
                continue
            if source.status == "error":
                errors += 1
            else:
                added += 1

        await self.refresh_sources()
        if errors and not added:
            self.app.notify(f"Could not import {errors} file(s)", severity="error")
        elif errors:
            self.app.notify(
                f"Added {added} source(s), {errors} failed", severity="warning"
            )
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
                self.run_worker(
                    self._remove_source(source.id or 0),
                    group=IO_GROUP,
                    exclusive=True,
                    exit_on_error=False,
                )

        self.app.push_screen(
            ConfirmDialog(f"Remove source '{source.filename}'?"),
            callback=_on_confirm,
        )

    async def _remove_source(self, source_id: int) -> None:
        if not self.repo:
            return
        try:
            await self.repo.delete_source(source_id)
        except Exception as e:  # noqa: BLE001
            self.app.notify(f"Could not remove source: {e}", severity="error")
        await self.refresh_sources()

    async def action_reimport_source(self) -> None:
        source = self._cursor_source()
        if not source or not self.repo or source.id is None:
            return
        self.run_worker(
            self._reimport(source),
            group=IO_GROUP,
            exclusive=True,
            exit_on_error=False,
        )

    async def _reimport(self, source: models.Source) -> None:
        if not self.repo or source.id is None:
            return
        full = await self.repo.get_source(source.id)
        if full is None:
            await self.refresh_sources()
            return

        p = Path(full.path)
        if not p.is_file():
            await self.repo.mark_source_error(full.id or 0, "File no longer exists")
            await self.refresh_sources()
            self.app.notify(f"File missing: {full.path}", severity="error")
            return

        try:
            content, size, ext, mime, err = await asyncio.to_thread(
                extract.extract_text, p
            )
        except Exception as e:  # noqa: BLE001 - parser exceptions become source errors
            content, size, ext, mime, err = "", 0, full.ext, full.mime, str(e)

        if err is None and not content.strip():
            err = process.NO_TEXT_ERROR

        token_estimate = estimate_tokens(content)
        chunks = chunk.chunk_text(content) if err is None and content else []
        await self.repo.replace_source_content(
            full.id or 0,
            content_text=content,
            token_estimate=token_estimate,
            size=size,
            mime=mime,
            ext=ext,
            filename=p.name,
            status="ok" if err is None else "error",
            error=err,
            chunks=list(enumerate(chunks)),
        )
        await self.refresh_sources()
        if err is not None:
            self.app.notify(f"Re-import failed: {err}", severity="error")

    async def action_preview_source(self) -> None:
        source = self._cursor_source()
        if not source or not self.repo or source.id is None:
            return
        full = await self.repo.get_source(source.id)
        text = (full.content_text if full else "") or source.error or "(no content)"
        if len(text) > MAX_PREVIEW_CHARS:
            text = text[:MAX_PREVIEW_CHARS] + "\n\n… (preview truncated)"
        from ..modals import SourcePreview

        self.app.push_screen(SourcePreview(source.filename, _safe_text(text)))

    # --- model ---------------------------------------------------------------

    async def action_model_picker(self) -> None:
        from ..modals import ModelPicker

        available = await self.app.get_models()  # type: ignore[attr-defined]
        if not available:
            self.app.notify(
                "No models available. Check your API key.", severity="warning"
            )
            return
        nb = await self.repo.get_notebook(self.notebook_id) if self.repo else None
        current = (nb.model if nb else None) or self.cfg().default_model

        def _on_dismiss(selection: object) -> None:
            if selection is None:
                return
            self.run_worker(
                self._apply_model(
                    selection.model_id,
                    selection.save_as_default,  # type: ignore[attr-defined]
                ),
                group=MODEL_GROUP,
                exclusive=True,
                exit_on_error=False,
            )

        self.app.push_screen(
            ModelPicker(available, current=current), callback=_on_dismiss
        )

    async def _apply_model(self, model_id: str, save_default: bool) -> None:
        self._model = model_id
        if self.repo:
            await self.repo.update_notebook(self.notebook_id, model=model_id)
        if save_default:
            self.cfg().default_model = model_id
            from ... import config

            try:
                config.save_config(self.cfg())
            except OSError as e:
                self.app.notify(f"Could not save default model: {e}", severity="error")
        self._update_footer()

    # --- chat ----------------------------------------------------------------

    async def _render_history(self) -> None:
        log = self.query_one("#chat-log", VerticalScroll)
        await log.remove_children()
        for msg in self._messages:
            await self._append_message(msg.role, msg.content, scroll=False)
        if self._messages:
            log.scroll_end(animate=False)

    async def _append_message(
        self, role: str, content: str, scroll: bool = True
    ) -> Markdown:
        log = self.query_one("#chat-log", VerticalScroll)
        safe = _safe_text(content)
        if role == "user":
            rendered = f"**You:** {safe}"
        elif role == "error":
            rendered = f"> ⚠️ {safe}"
        else:
            rendered = safe
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

    async def on_sources_table_toggle_source(
        self, event: SourcesTable.ToggleSource
    ) -> None:
        event.stop()
        await self.action_toggle_source()

    async def on_sources_table_remove_source(
        self, event: SourcesTable.RemoveSource
    ) -> None:
        event.stop()
        await self.action_remove_source()

    async def on_sources_table_reimport_source(
        self, event: SourcesTable.ReimportSource
    ) -> None:
        event.stop()
        await self.action_reimport_source()

    async def on_data_table_row_selected(self, event) -> None:
        if event.data_table.id == "sources":
            event.stop()
            await self.action_preview_source()

    def _set_generating(self, value: bool) -> None:
        self._generating = value
        with suppress(Exception):  # screen may be unmounting
            self.query_one("#chat-input", ChatInput).disabled = value
            self._update_footer()

    async def _start_generation(self, question: str) -> None:
        if self._generating:
            self.app.notify(
                "Still generating. Press Esc to cancel.", severity="warning"
            )
            return
        if not self.repo or not question.strip():
            return

        self._set_generating(True)
        input_widget = self.query_one("#chat-input", ChatInput)
        try:
            prior_history = list(self._messages)
            user_msg = models.Message(
                notebook_id=self.notebook_id, role="user", content=question
            )
            user_msg = await self.repo.add_message(user_msg)
            self._messages.append(user_msg)
            await self._append_message("user", question)
            input_widget.clear()

            self._gen_worker = self.run_worker(
                self._generate(question, prior_history),
                group=CHAT_GROUP,
                exclusive=True,
                exit_on_error=False,
                name="chat",
            )
        except Exception:
            self._set_generating(False)
            raise

    async def _generate(
        self, question: str, prior_history: list[models.Message]
    ) -> None:
        assert self.repo is not None
        repo = self.repo
        model = self._model or self.cfg().default_model
        if not model:
            await self._finish_with_error(
                "No model selected. Press Ctrl+M to choose one."
            )
            return
        if not self.cfg().api_key:
            await self._finish_with_error(
                "No API key configured. Set OPENROUTER_API_KEY or press Ctrl+K."
            )
            return

        try:
            sources = await repo.list_sources_for_context(self.notebook_id)
            budget = self.cfg().context_token_budget
            _fixed, _history_budget, source_budget = context_mod.allocate_budgets(
                self.cfg().system_prompt,
                question,
                budget,
                ANSWER_RESERVE_TOKENS,
            )
            match = context_mod.sanitize_fts_query(question)
            hits: list[context_mod.ChunkHit] = []
            total_source_tokens = sum(estimate_tokens(s.content_text) for s in sources)
            if total_source_tokens > source_budget and match:
                raw_hits = await repo.search_chunks(self.notebook_id, match)
                hits = [
                    context_mod.ChunkHit(sid, filename, text)
                    for sid, filename, text in raw_hits
                ]

            ctx = context_mod.build_context(
                question,
                sources,
                prior_history,
                self.cfg().system_prompt,
                budget,
                search_chunks=lambda _q: hits,
                answer_reserve=ANSWER_RESERVE_TOKENS,
            )
        except Exception as e:  # noqa: BLE001
            await self._finish_with_error(f"Could not build context: {e}")
            return

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
        parts: list[str] = []
        client: openrouter.OpenRouterClient | None = None
        try:
            client = openrouter.OpenRouterClient(self.cfg().api_key)
            log = self.query_one("#chat-log", VerticalScroll)
            last_paint = 0.0
            async for token in client.stream_chat(model, messages):
                parts.append(token)
                now = time.monotonic()
                if now - last_paint >= 0.05:
                    assistant_md.update("".join(parts))
                    log.scroll_end(animate=False)
                    last_paint = now

            final = "".join(parts)
            if not final:
                raise openrouter.OpenRouterError(
                    "The model returned an empty response."
                )
            assistant_md.update(final)
        except asyncio.CancelledError:
            final = "".join(parts)
            if final:
                partial = await self._save_partial(final, "\n\n_(interrupted)_")
                assistant_md.update(partial)
            raise
        except openrouter.OpenRouterError as e:
            final = "".join(parts)
            if final:
                partial = await self._save_partial(final, "\n\n_(interrupted: error)_")
                assistant_md.update(partial)
            else:
                await self._record_error(e.message)
            await self._append_message("error", e.message)
        except Exception as e:  # noqa: BLE001
            final = "".join(parts)
            if final:
                partial = await self._save_partial(final, "\n\n_(interrupted: error)_")
                assistant_md.update(partial)
            await self._record_error(f"Unexpected error: {e}")
            await self._append_message("error", f"Unexpected error: {e}")
        else:
            saved = await repo.add_message(
                models.Message(
                    notebook_id=self.notebook_id,
                    role="assistant",
                    content=final,
                    model=model,
                )
            )
            self._messages.append(saved)
        finally:
            if client is not None:
                await client.aclose()
            self._gen_worker = None
            self._set_generating(False)

    async def _save_partial(self, text: str, suffix: str) -> str:
        assert self.repo is not None
        partial = text + suffix
        saved = await self.repo.add_message(
            models.Message(
                notebook_id=self.notebook_id,
                role="assistant",
                content=partial,
                model=self._model or self.cfg().default_model,
            )
        )
        self._messages.append(saved)
        return partial

    async def _finish_with_error(self, message: str) -> None:
        await self._append_message("error", message)
        await self._record_error(message)
        self._gen_worker = None
        self._set_generating(False)

    async def _record_error(self, message: str) -> None:
        if not self.repo:
            return
        saved = await self.repo.add_message(
            models.Message(
                notebook_id=self.notebook_id,
                role="error",
                content=_safe_text(message),
            )
        )
        self._messages.append(saved)

    # --- misc actions --------------------------------------------------------

    async def action_clear_chat(self) -> None:
        if not self.repo:
            return
        from ..modals import ConfirmDialog

        def _on_confirm(confirmed: bool) -> None:
            if confirmed:
                self.run_worker(
                    self._clear_chat(),
                    group=UI_GROUP,
                    exclusive=True,
                    exit_on_error=False,
                )

        self.app.push_screen(
            ConfirmDialog("Clear this notebook's chat history?"), callback=_on_confirm
        )

    async def _clear_chat(self) -> None:
        worker = self._gen_worker
        if worker is not None:
            worker.cancel()
            try:
                await worker.wait()
            except (asyncio.CancelledError, WorkerCancelled):
                pass
            self._gen_worker = None
        self._set_generating(False)
        if self.repo:
            await self.repo.clear_messages(self.notebook_id)
        self._messages = []
        await self._render_history()

    async def action_help(self) -> None:
        focused = self.app.focused
        if isinstance(focused, ChatInput):
            focused.insert("?")
            return
        from ..modals import HelpScreen

        self.app.push_screen(HelpScreen())

    async def action_escape(self) -> None:
        if self._gen_worker is not None:
            self._gen_worker.cancel()
            self.app.notify("Generation cancelled")
            return
        await self.app.pop_screen()
