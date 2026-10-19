from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Static


class SourcePreview(ModalScreen[None]):
    BINDINGS: ClassVar[list[Binding]] = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, title: str, content: str) -> None:
        super().__init__()
        self._title = title
        self._content = content

    def compose(self) -> ComposeResult:
        with Vertical(id="source-preview"):
            with VerticalScroll():
                yield Static(self._title, id="preview-title", markup=False)
                # Source text is untrusted: never render it as Markdown.
                yield Static(self._content, id="preview-body", markup=False)
            yield Button("Close", id="close", variant="primary")

    def action_cancel(self) -> None:
        self.dismiss(None)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(None)
