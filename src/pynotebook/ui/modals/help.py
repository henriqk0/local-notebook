from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Markdown

HELP_TEXT = """
# pynotebook help

## Notebook list
- `n` new notebook
- `e` edit notebook
- `d` delete notebook (confirm)
- `enter` open notebook
- `/` filter notebooks
- `m` choose default model
- `ctrl+k` change API key
- `q` quit

## Notebook
### Sources (Tab to focus the sources table)
- `a` add source (ranger)
- `space` toggle enabled
- `x` remove source (confirm)
- `r` re-import from disk
- `enter` preview text

### Chat
- `Enter` send message
- `Shift+Enter` / `Ctrl+J` newline
- `Ctrl+M` model picker
- `Ctrl+L` clear chat (confirm)
- `Esc` cancel generation / back to notebook list
- `?` this help
"""


class HelpScreen(ModalScreen[None]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def compose(self) -> ComposeResult:
        with Vertical(id="help"):
            with VerticalScroll():
                yield Markdown(HELP_TEXT)
            yield Button("Close", id="close", variant="primary")

    def action_cancel(self) -> None:
        self.dismiss(None)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(None)
