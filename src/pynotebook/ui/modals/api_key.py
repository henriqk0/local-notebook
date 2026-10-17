from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label, Static


class ApiKeyDialog(ModalScreen[str | None]):
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
            yield Button("Save", id="save")
            yield Button("Skip", id="skip")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "save":
            value = self.query_one("#key", Input).value.strip()
            self.dismiss(value or None)
        else:
            self.dismiss(None)
