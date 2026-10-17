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
            yield DataTable(id="notebooks", cursor_type="row")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns("Name", "Description", "#Sources", "Last Updated")

    async def action_new_notebook(self) -> None:
        await self.app.action_new_notebook()  # type: ignore[attr-defined]

    async def action_edit_notebook(self) -> None:
        await self.app.action_edit_notebook()  # type: ignore[attr-defined]

    async def action_delete_notebook(self) -> None:
        await self.app.action_delete_notebook()  # type: ignore[attr-defined]

    async def action_open_notebook(self) -> None:
        await self.app.action_open_notebook()  # type: ignore[attr-defined]

    async def action_filter(self) -> None:
        await self.app.action_filter()  # type: ignore[attr-defined]

    async def action_default_model(self) -> None:
        await self.app.action_default_model()  # type: ignore[attr-defined]

    def action_quit(self) -> None:
        self.app.exit()

    async def on_data_table_row_selected(self, event) -> None:
        if event.data_table.id == "notebooks":
            event.stop()
            await self.action_open_notebook()
