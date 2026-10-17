from __future__ import annotations

from dataclasses import dataclass

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Input, Label

from ...llm.models_cache import ModelInfo


@dataclass
class ModelSelection:
    model_id: str
    save_as_default: bool


class ModelPicker(ModalScreen[ModelSelection | None]):
    def __init__(self, models: list[ModelInfo], current: str | None = None) -> None:
        super().__init__()
        self._models = models
        self._current = current
        self._filtered = list(models)

    def compose(self) -> ComposeResult:
        with Vertical(id="model-picker"):
            yield Label("Select a model (type to search)")
            yield Input(placeholder="Search by id or name", id="model-search")
            yield DataTable(id="model-table")
            with Horizontal():
                yield Button("Use", id="use")
                yield Button("Set as default", id="use-default")
                yield Button("Cancel", id="cancel")

    def on_mount(self) -> None:
        table = self.query_one("#model-table", DataTable)
        table.add_columns("Model", "Context", "Prompt $/1M", "Compl $/1M")
        self._populate()

    def _populate(self) -> None:
        table = self.query_one("#model-table", DataTable)
        table.clear()
        for m in self._filtered:
            free = " (free)" if m.is_free else ""
            table.add_row(
                f"{m.id}{free}",
                str(m.context_length),
                f"{m.prompt_price:.2f}",
                f"{m.completion_price:.2f}",
                key=m.id,
            )

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "model-search":
            from ...llm.models_cache import ModelCache

            self._filtered = ModelCache.filter_models(self._models, event.value)
            self._populate()

    def _selected_model(self) -> str | None:
        table = self.query_one("#model-table", DataTable)
        if table.cursor_row is None:
            return None
        if table.cursor_row >= len(self._filtered):
            return None
        return self._filtered[table.cursor_row].id

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "cancel":
            self.dismiss(None)
            return
        model_id = self._selected_model()
        if not model_id:
            return
        if event.button.id == "use-default":
            self.dismiss(ModelSelection(model_id, save_as_default=True))
        else:
            self.dismiss(ModelSelection(model_id, save_as_default=False))
