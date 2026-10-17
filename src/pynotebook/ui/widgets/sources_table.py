from __future__ import annotations

from typing import ClassVar

from textual.binding import Binding
from textual.message import Message
from textual.widgets import DataTable


class SourcesTable(DataTable):
    """Sources list that owns the source-management keybindings.

    Keeping the bindings here (rather than on the screen) means single-letter
    keys don't hijack typing in the chat input.
    """

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("a", "add_source", "Add source"),
        Binding("space", "toggle_source", "Toggle"),
        Binding("x", "remove_source", "Remove"),
        Binding("r", "reimport_source", "Re-import"),
    ]

    class AddSource(Message):
        pass

    class ToggleSource(Message):
        pass

    class RemoveSource(Message):
        pass

    class ReimportSource(Message):
        pass

    def action_add_source(self) -> None:
        self.post_message(self.AddSource())

    def action_toggle_source(self) -> None:
        self.post_message(self.ToggleSource())

    def action_remove_source(self) -> None:
        self.post_message(self.RemoveSource())

    def action_reimport_source(self) -> None:
        self.post_message(self.ReimportSource())
