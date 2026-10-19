from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label, Static


class NotebookForm(ModalScreen[tuple[str, str] | None]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def __init__(
        self, name: str = "", description: str = "", title: str = "Notebook"
    ) -> None:
        super().__init__()
        self._name = name
        self._description = description
        self._title = title

    def compose(self) -> ComposeResult:
        with Vertical(id="form"):
            yield Label(self._title)
            yield Label("Name")
            yield Input(self._name, id="name", placeholder="Name")
            yield Label("Description")
            yield Input(self._description, id="description", placeholder="Description")
            yield Static("", id="form-error")
            yield Button("Save", id="save", variant="primary")
            yield Button("Cancel", id="cancel")

    def on_mount(self) -> None:
        self.query_one("#name", Input).focus()

    def _save(self) -> None:
        name = self.query_one("#name", Input).value
        desc = self.query_one("#description", Input).value
        if not name.strip():
            self.query_one("#form-error", Static).update("Name is required.")
            return
        self.dismiss((name, desc))

    def action_cancel(self) -> None:
        self.dismiss(None)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        self._save()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "save":
            self._save()
        else:
            self.dismiss(None)
