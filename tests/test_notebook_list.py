import pytest

from pynotebook.ui.screens import NotebookListScreen


@pytest.mark.asyncio
async def test_notebook_list_screen_mounts():
    from textual.app import App

    class DummyApp(App[None]):
        async def on_mount(self) -> None:
            await self.push_screen(NotebookListScreen())

    app = DummyApp()
    async with app.run_test():
        assert app.screen is not None
        assert app.screen.__class__.__name__ == "NotebookListScreen"
