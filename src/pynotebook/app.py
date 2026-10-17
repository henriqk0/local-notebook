from __future__ import annotations

from typing import ClassVar

from textual.app import App
from textual.binding import Binding

from . import config, repo
from .llm import models_cache, openrouter
from .ui.modals import ConfirmDialog, ModelPicker, NotebookForm
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
        self._model_cache = models_cache.ModelCache(config.get_data_dir() / "models_cache.json")
        self._filter_text = ""

    async def on_mount(self) -> None:
        from .ui.modals import ApiKeyDialog

        data_dir = config.get_data_dir()
        db_path = data_dir / "pynotebook.db"
        self.repo = await repo.Repository.from_path(db_path)
        screen = NotebookListScreen()
        await self.push_screen(screen)
        await self._refresh_notebook_list(screen)
        if not self.cfg.api_key:
            self.push_screen(ApiKeyDialog(), callback=self._on_api_key)

    def _on_api_key(self, value: str | None) -> None:
        if value:
            self.cfg.api_key = value
            config.save_config(self.cfg)
            self.notify("API key saved")

    async def get_models(self, force_refresh: bool = False) -> list[models_cache.ModelInfo]:
        if not force_refresh:
            cached = self._model_cache.load()
            if cached:
                return cached
        if not self.cfg.api_key:
            return self._model_cache.load() or []
        client = openrouter.OpenRouterClient(self.cfg.api_key)
        try:
            raw = await client.list_models()
        except openrouter.OpenRouterError:
            return self._model_cache.load() or []
        finally:
            await client.aclose()
        models = [models_cache.ModelInfo.from_api(m) for m in raw]
        self._model_cache.save(models)
        return models

    async def action_default_model(self) -> None:
        models = await self.get_models()
        if not models:
            self.notify("No models available. Check your API key.", severity="warning")
            return

        def _on_dismiss(selection: object) -> None:
            if selection is None:
                return
            model_id = getattr(selection, "model_id", None)
            if model_id:
                self.cfg.default_model = model_id
                config.save_config(self.cfg)
                self.notify(f"Default model set to {model_id}")

        self.push_screen(ModelPicker(models, current=self.cfg.default_model), callback=_on_dismiss)

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
        from .ui.screens import NotebookScreen

        await self.push_screen(NotebookScreen(nb.id or 0, nb.name))

    async def action_filter(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen):
            return
        from .ui.modals import PathInputDialog

        def _on_dismiss(value: str | None) -> None:
            if value is None:
                return
            self._filter_text = value.strip()
            self.run_worker(self._refresh_notebook_list(current), exclusive=True)

        self.push_screen(
            PathInputDialog("Filter notebooks (leave empty to clear):"), callback=_on_dismiss
        )

    async def _refresh_notebook_list(self, screen: NotebookListScreen) -> None:
        if not self.repo:
            return
        nbs = await self.repo.list_notebooks()
        if self._filter_text:
            needle = self._filter_text.lower()
            nbs = [n for n in nbs if needle in n.name.lower() or needle in n.description.lower()]
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

        if not nbs:
            if self._filter_text:
                screen.set_empty_message(f"No notebooks match '{self._filter_text}'. Press / to clear.")
            else:
                screen.set_empty_message("No notebooks yet. Press 'n' to create one.")
        else:
            screen.set_empty_message("")


def main() -> None:
    app = PyNotebookApp()
    app.run()


if __name__ == "__main__":
    main()
