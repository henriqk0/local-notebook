from __future__ import annotations

from typing import ClassVar

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header


class NotebookListScreen(Screen):
    BINDINGS: ClassVar[list[Binding]] = [
        Binding("n", "new_notebook", "New"),
        Binding("e", "edit_notebook", "Edit"),
        Binding("d", "delete_notebook", "Delete"),
        Binding("enter", "open_notebook", "Open"),
        Binding("/", "filter", "Filter"),
        Binding("m", "default_model", "Default Model"),
        Binding("q", "quit", "Quit"),
    ]

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical():
            yield DataTable(id="notebooks")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns("Name", "Description", "#Sources", "Last Updated")
