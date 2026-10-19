from __future__ import annotations

from contextlib import suppress
from typing import ClassVar

from textual.app import App
from textual.binding import Binding
from textual.widgets import DataTable

from . import config, repo
from .llm import models_cache, openrouter
from .ui.modals import ConfirmDialog, ModelPicker, NotebookForm
from .ui.screens import NotebookListScreen

APP_WORKER_GROUP = "app"


class PyNotebookApp(App[None]):
    CSS_PATH = "ui/styles.tcss"
    BINDINGS: ClassVar[list[Binding]] = [
        Binding("q", "quit", "Quit"),
        Binding("ctrl+k", "api_key", "API key"),
    ]

    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.repo: repo.Repository | None = None
        self.cfg = config.load_config()
        self._model_cache = models_cache.ModelCache(
            config.get_data_dir() / "models_cache.json"
        )
        self._filter_text = ""

    async def on_mount(self) -> None:
        from .ui.modals import ApiKeyDialog

        data_dir = config.get_data_dir()
        db_path = data_dir / "pynotebook.db"
        screen = NotebookListScreen()
        await self.push_screen(screen)
        try:
            self.repo = await repo.Repository.from_path(db_path)
        except Exception as e:  # noqa: BLE001 - surface DB failures instead of crashing
            self.notify(
                f"Could not open database: {e}",
                severity="error",
                timeout=10,
            )
            screen.set_empty_message("Database unavailable.")
            return
        await self._refresh_notebook_list(screen)
        if not self.cfg.api_key:
            self.push_screen(ApiKeyDialog(), callback=self._on_api_key)

    async def _shutdown(self) -> None:
        # aiosqlite's worker thread is non-daemon, so the connection must be
        # closed or the process will hang after the UI exits.
        repository = self.repo
        self.repo = None
        if repository is not None:
            with suppress(Exception):
                await repository.close()
        await super()._shutdown()

    def _on_api_key(self, value: str | None) -> None:
        if not value:
            return
        self.cfg.set_api_key(value)
        try:
            config.save_config(self.cfg)
        except OSError as e:
            self.notify(f"Could not save API key: {e}", severity="error")
            return
        self.notify("API key saved")

    async def action_api_key(self) -> None:
        from .ui.modals import ApiKeyDialog

        self.push_screen(ApiKeyDialog(), callback=self._on_api_key)

    async def get_models(
        self, force_refresh: bool = False
    ) -> list[models_cache.ModelInfo]:
        if not force_refresh:
            cached = self._model_cache.load()
            if cached:
                return cached
        if not self.cfg.api_key:
            return self._model_cache.load() or []
        client = openrouter.OpenRouterClient(self.cfg.api_key)
        try:
            raw = await client.list_models()
        except (openrouter.OpenRouterError, ValueError, TypeError) as e:
            message = getattr(e, "message", str(e))
            self.notify(f"Could not load models: {message}", severity="warning")
            return self._model_cache.load() or []
        finally:
            await client.aclose()
        models = [models_cache.ModelInfo.from_api(m) for m in raw]
        try:
            self._model_cache.save(models)
        except OSError:
            # The cache is best-effort; a read-only data dir must not break chat.
            pass
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
                try:
                    config.save_config(self.cfg)
                except OSError as e:
                    self.notify(f"Could not save default model: {e}", severity="error")
                    return
                self.notify(f"Default model set to {model_id}")

        self.push_screen(
            ModelPicker(models, current=self.cfg.default_model),
            callback=_on_dismiss,
        )

    async def action_new_notebook(self) -> None:
        def _on_dismiss(result: tuple[str, str] | None) -> None:
            if result is not None and result[0].strip():
                name, desc = result[0].strip(), result[1].strip()
                self.run_worker(
                    self._create_notebook(name, desc),
                    group=APP_WORKER_GROUP,
                    exclusive=True,
                    exit_on_error=False,
                )

        self.push_screen(NotebookForm(), callback=_on_dismiss)

    async def _create_notebook(self, name: str, description: str) -> None:
        if not self.repo:
            return
        try:
            await self.repo.create_notebook(name, description)
        except Exception as e:  # noqa: BLE001
            self.notify(f"Could not create notebook: {e}", severity="error")
            return
        current = self.screen
        if isinstance(current, NotebookListScreen):
            await self._refresh_notebook_list(current)

    async def action_edit_notebook(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen) or not self.repo:
            return
        table = current.query_one("DataTable")
        notebook_id = self._selected_notebook_id(table)
        if notebook_id is None:
            return
        nb = await self.repo.get_notebook(notebook_id)
        if nb is None or nb.id is None:
            return

        def _on_dismiss(result: tuple[str, str] | None) -> None:
            if result is not None:
                name = result[0].strip() or nb.name
                desc = result[1].strip()
                self.run_worker(
                    self._update_notebook(nb.id, name, desc),
                    group=APP_WORKER_GROUP,
                    exclusive=True,
                    exit_on_error=False,
                )

        self.push_screen(
            NotebookForm(
                name=nb.name, description=nb.description, title="Edit Notebook"
            ),
            callback=_on_dismiss,
        )

    async def _update_notebook(
        self, notebook_id: int | None, name: str, description: str
    ) -> None:
        if not self.repo or notebook_id is None:
            return
        try:
            await self.repo.update_notebook(
                notebook_id, name=name, description=description
            )
        except Exception as e:  # noqa: BLE001
            self.notify(f"Could not update notebook: {e}", severity="error")
            return
        current = self.screen
        if isinstance(current, NotebookListScreen):
            await self._refresh_notebook_list(current)

    async def action_delete_notebook(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen) or not self.repo:
            return
        table = current.query_one("DataTable")
        notebook_id = self._selected_notebook_id(table)
        if notebook_id is None:
            return
        nb = await self.repo.get_notebook(notebook_id)
        if nb is None:
            return

        def _on_confirm(confirmed: bool) -> None:
            if confirmed:
                self.run_worker(
                    self._delete_notebook(notebook_id),
                    group=APP_WORKER_GROUP,
                    exclusive=True,
                    exit_on_error=False,
                )

        self.push_screen(
            ConfirmDialog(f"Delete notebook '{nb.name}'?"),
            callback=_on_confirm,
        )

    async def _delete_notebook(self, notebook_id: int | None) -> None:
        if not self.repo or notebook_id is None:
            return
        try:
            await self.repo.delete_notebook(notebook_id)
        except Exception as e:  # noqa: BLE001
            self.notify(f"Could not delete notebook: {e}", severity="error")
            return
        current = self.screen
        if isinstance(current, NotebookListScreen):
            await self._refresh_notebook_list(current)

    async def action_open_notebook(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen) or not self.repo:
            return
        table = current.query_one("DataTable")
        notebook_id = self._selected_notebook_id(table)
        if notebook_id is None:
            return
        nb = await self.repo.get_notebook(notebook_id)
        if nb is None or nb.id is None:
            return
        from .ui.screens import NotebookScreen

        await self.push_screen(NotebookScreen(nb.id, nb.name))

    def _selected_notebook_id(self, table: DataTable) -> int | None:
        if table.cursor_row is None:
            return None
        try:
            row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key
            return int(row_key.value)
        except (TypeError, ValueError):
            return None

    async def action_filter(self) -> None:
        current = self.screen
        if not isinstance(current, NotebookListScreen):
            return
        from .ui.modals import PathInputDialog

        def _on_dismiss(value: str | None) -> None:
            if value is None:
                return
            self._filter_text = value.strip()
            self.run_worker(
                self._refresh_notebook_list(current),
                group=APP_WORKER_GROUP,
                exclusive=True,
                exit_on_error=False,
            )

        self.push_screen(
            PathInputDialog("Filter notebooks (leave empty to clear):"),
            callback=_on_dismiss,
        )

    async def _refresh_notebook_list(self, screen: NotebookListScreen) -> None:
        if not self.repo:
            return
        try:
            rows = await self.repo.list_notebooks_with_counts()
        except Exception as e:  # noqa: BLE001
            self.notify(f"Could not load notebooks: {e}", severity="error")
            return
        if self._filter_text:
            needle = self._filter_text.lower()
            rows = [
                (nb, count)
                for nb, count in rows
                if needle in nb.name.lower() or needle in nb.description.lower()
            ]
        table = screen.query_one("DataTable")
        table.clear()
        for nb, source_count in rows:
            updated = nb.updated_at.strftime("%Y-%m-%d %H:%M") if nb.updated_at else ""
            table.add_row(
                nb.name,
                nb.description or "",
                str(source_count),
                updated,
                key=str(nb.id) if nb.id is not None else nb.name,
            )

        if not rows:
            if self._filter_text:
                screen.set_empty_message(
                    f"No notebooks match '{self._filter_text}'. Press / to clear."
                )
            else:
                screen.set_empty_message("No notebooks yet. Press 'n' to create one.")
        else:
            screen.set_empty_message("")


def main() -> None:
    app = PyNotebookApp()
    app.run()


if __name__ == "__main__":
    main()
