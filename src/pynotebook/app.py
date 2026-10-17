from __future__ import annotations

from typing import ClassVar

from textual.app import App
from textual.binding import Binding

from . import config, repo
from .ui.modals import ConfirmDialog, NotebookForm
from .ui.screens import NotebookListScreen


class PyNotebookApp(App[None]):
    CSS_PATH = "ui/styles.tcss"
    BINDINGS: ClassVar[list[Binding]] = [
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.repo: repo.Repository | None = None
        self.cfg = config.load_config()

    async def on_mount(self) -> None:
        data_dir = config.get_data_dir()
        db_path = data_dir / "pynotebook.db"
        self.repo = await repo.Repository.from_path(db_path)
        screen = NotebookListScreen()
        await self.push_screen(screen)
        await self._refresh_notebook_list(screen)

    async def action_new_notebook(self) -> None:
        def _on_dismiss(result: tuple[str, str] | None) -> None:
            if result is not None and result[0].strip():
                name, desc = result[0].strip(), result[1].strip()
                self.run_worker(self._create_notebook(name, desc), exclusive=True)

        self.push_screen(NotebookForm(), callback=_on_dismiss)

    async def _create_notebook(self, name: str, description: str) -> None:
        if self.repo:
            await self.repo.create_notebook(name, description)
            current = self.screen
            if isinstance(current, NotebookListScreen):
                await self._refresh_notebook_list(current)

    async def action_edit_notebook(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen):
            return
        table = current.query_one("DataTable")
        if table.cursor_row is None:
            return
        if not self.repo:
            return
        nbs = await self.repo.list_notebooks()
        if not nbs or table.cursor_row >= len(nbs):
            return
        nb = nbs[table.cursor_row]

        def _on_dismiss(result: tuple[str, str] | None) -> None:
            if result is not None:
                name = result[0].strip() or nb.name
                desc = result[1].strip()
                self.run_worker(self._update_notebook(nb.id, name, desc), exclusive=True)

        self.push_screen(
            NotebookForm(name=nb.name, description=nb.description, title="Edit Notebook"),
            callback=_on_dismiss,
        )

    async def _update_notebook(self, notebook_id: int | None, name: str, description: str) -> None:
        if self.repo and notebook_id is not None:
            await self.repo.update_notebook(notebook_id, name=name, description=description)
            current = self.screen
            if isinstance(current, NotebookListScreen):
                await self._refresh_notebook_list(current)

    async def action_delete_notebook(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen):
            return
        if not self.repo:
            return
        nbs = await self.repo.list_notebooks()
        table = current.query_one("DataTable")
        if not nbs or table.cursor_row is None or table.cursor_row >= len(nbs):
            return
        nb = nbs[table.cursor_row]

        def _on_confirm(confirmed: bool) -> None:
            if confirmed:
                self.run_worker(self._delete_notebook(nb.id), exclusive=True)

        self.push_screen(ConfirmDialog(f"Delete notebook '{nb.name}'?"), callback=_on_confirm)

    async def _delete_notebook(self, notebook_id: int | None) -> None:
        if self.repo and notebook_id is not None:
            await self.repo.delete_notebook(notebook_id)
            current = self.screen
            if isinstance(current, NotebookListScreen):
                await self._refresh_notebook_list(current)

    async def action_open_notebook(self) -> None:
        pass

    async def action_filter(self) -> None:
        pass

    async def action_default_model(self) -> None:
        pass

    async def _refresh_notebook_list(self, screen: NotebookListScreen) -> None:
        if not self.repo:
            return
        nbs = await self.repo.list_notebooks()
        table = screen.query_one("DataTable")
        table.clear()
        for nb in nbs:
            sources_count = 0
            try:
                sources = await self.repo.list_sources(nb.id or 0)
                sources_count = len(sources)
            except (OSError, ValueError):
                sources_count = 0
            updated = nb.updated_at.strftime("%Y-%m-%d %H:%M") if nb.updated_at else ""
            table.add_row(nb.name, nb.description or "", str(sources_count), updated, key=str(nb.id) if nb.id else nb.name)


def main() -> None:
    app = PyNotebookApp()
    app.run()


if __name__ == "__main__":
    main()
