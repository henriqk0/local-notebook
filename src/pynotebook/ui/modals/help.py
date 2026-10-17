from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
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
- `q` quit

## Notebook
### Sources
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
    def compose(self) -> ComposeResult:
        with Vertical(id="help"):
            yield Markdown(HELP_TEXT)
            yield Button("Close", id="close")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(None)
