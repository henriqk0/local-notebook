from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label


class PathInputDialog(ModalScreen[str | None]):
    def __init__(self, message: str = "Enter a file path:") -> None:
        super().__init__()
        self._message = message

    def compose(self) -> ComposeResult:
        with Vertical(id="path-input"):
            yield Label(self._message)
            yield Input(placeholder="~/path/to/file or glob", id="path")
            yield Button("OK", id="ok")
            yield Button("Cancel", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "ok":
            self.dismiss(self.query_one("#path", Input).value)
        else:
            self.dismiss(None)
