from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Label


class ConfirmDialog(ModalScreen[bool]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, message: str) -> None:
        super().__init__()
        self._message = message

    def compose(self) -> ComposeResult:
        with Vertical(id="confirm"):
            yield Label(self._message)
            yield Button("Confirm", id="yes", variant="error")
            yield Button("Cancel", id="no", variant="primary")

    def on_mount(self) -> None:
        self.query_one("#no", Button).focus()

    def action_cancel(self) -> None:
        self.dismiss(False)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "yes")
