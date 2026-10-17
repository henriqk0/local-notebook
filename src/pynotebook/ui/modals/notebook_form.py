from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label, Static


class NotebookForm(ModalScreen[tuple[str, str] | None]):
    def __init__(self, name: str = "", description: str = "", title: str = "Notebook") -> None:
        super().__init__()
        self._name = name
        self._description = description
        self._title = title

    def compose(self) -> ComposeResult:
        with Vertical(id="form"):
            yield Label(self._title)
            yield Input(self._name, id="name", placeholder="Name")
            yield Input(self._description, id="description", placeholder="Description")
            yield Static()
            yield Button("Save", id="save")
            yield Button("Cancel", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "save":
            name = self.query_one("#name", Input).value
            desc = self.query_one("#description", Input).value
            self.dismiss((name, desc))
        else:
            self.dismiss(None)
