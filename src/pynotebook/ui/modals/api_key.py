from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label, Static


class ApiKeyDialog(ModalScreen[str | None]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, message: str | None = None) -> None:
        super().__init__()
        self._message = message or (
            "Enter your OpenRouter API key. It will be saved to "
            "~/.config/pynotebook/config.toml with mode 0600."
        )

    def compose(self) -> ComposeResult:
        with Vertical(id="api-key"):
            yield Label("OpenRouter API key")
            yield Static(self._message)
            yield Input(placeholder="sk-or-...", password=True, id="key")
            yield Button("Save", id="save", variant="primary")
            yield Button("Skip", id="skip")

    def on_mount(self) -> None:
        self.query_one("#key", Input).focus()

    def _save(self) -> None:
        value = self.query_one("#key", Input).value.strip()
        self.dismiss(value or None)

    def action_cancel(self) -> None:
        self.dismiss(None)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        self._save()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "save":
            self._save()
        else:
            self.dismiss(None)
