from pathlib import Path

import pytest
from textual.app import App

from pynotebook import repo
from pynotebook.ui.screens import NotebookScreen


class DummyApp(App[None]):
    def __init__(self, repository):
        super().__init__()
        self.repo = repository
        from pynotebook import config

        self.cfg = config.Config()

    async def on_mount(self) -> None:
        await self.push_screen(NotebookScreen(1, "Test Notebook"))


@pytest.mark.asyncio
async def test_notebook_screen_mounts(tmp_path: Path):
    r = await repo.Repository.from_path(tmp_path / "t.db")
    await r.create_notebook("N1")
    app = DummyApp(r)
    async with app.run_test():
        screen = app.screen
        assert screen.__class__.__name__ == "NotebookScreen"
        # sources table exists
        assert screen.query_one("#sources") is not None
    await r.close()
