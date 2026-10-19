from __future__ import annotations

from dataclasses import dataclass
from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Input, Label

from ...llm.models_cache import ModelCache, ModelInfo


@dataclass
class ModelSelection:
    model_id: str
    save_as_default: bool


class ModelPicker(ModalScreen[ModelSelection | None]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, models: list[ModelInfo], current: str | None = None) -> None:
        super().__init__()
        self._models = sorted(models, key=lambda m: m.id.lower())
        self._current = current
        self._filtered = list(self._models)

    def compose(self) -> ComposeResult:
        with Vertical(id="model-picker"):
            yield Label("Select a model (type to search, Enter to use)")
            yield Input(placeholder="Search by id or name", id="model-search")
            yield DataTable(id="model-table")
            with Horizontal():
                yield Button("Use", id="use", variant="primary")
                yield Button("Set as default", id="use-default")
                yield Button("Cancel", id="cancel")

    def on_mount(self) -> None:
        table = self.query_one("#model-table", DataTable)
        table.add_columns("Model", "Context", "Prompt $/1M", "Compl $/1M")
        self.query_one("#model-search", Input).focus()
        self._populate()

    def _populate(self) -> None:
        table = self.query_one("#model-table", DataTable)
        table.clear()
        current_row = 0
        for index, m in enumerate(self._filtered):
            free = " (free)" if m.is_free else ""
            marker = "→ " if m.id == self._current else "  "
            table.add_row(
                f"{marker}{m.id}{free}",
                str(m.context_length),
                f"{m.prompt_price * 1_000_000:,.2f}",
                f"{m.completion_price * 1_000_000:,.2f}",
                key=m.id,
            )
            if m.id == self._current:
                current_row = index
        if self._filtered:
            table.move_cursor(row=min(current_row, len(self._filtered) - 1))

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "model-search":
            self._filtered = ModelCache.filter_models(self._models, event.value)
            self._populate()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "model-search":
            self._choose(save_as_default=False)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        if event.data_table.id == "model-table":
            self._choose(save_as_default=False)

    def _selected_model(self) -> str | None:
        table = self.query_one("#model-table", DataTable)
        if table.cursor_row is None or table.cursor_row >= len(self._filtered):
            return None
        return self._filtered[table.cursor_row].id

    def _choose(self, *, save_as_default: bool) -> None:
        model_id = self._selected_model()
        if not model_id:
            return
        self.dismiss(ModelSelection(model_id, save_as_default=save_as_default))

    def action_cancel(self) -> None:
        self.dismiss(None)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "cancel":
            self.dismiss(None)
            return
        self._choose(save_as_default=event.button.id == "use-default")
