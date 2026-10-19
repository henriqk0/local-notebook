import pytest
from textual.app import App
from textual.widgets import Static

from pynotebook.ui.modals import SourcePreview


class PreviewApp(App[None]):
    async def on_mount(self) -> None:
        self.push_screen(
            SourcePreview(
                "notes.txt", "# not a heading\n[link](http://example.invalid)"
            )
        )


@pytest.mark.asyncio
async def test_source_preview_renders_untrusted_text_plainly():
    app = PreviewApp()
    async with app.run_test() as pilot:
        body = app.screen.query_one("#preview-body", Static)
        assert body.content == "# not a heading\n[link](http://example.invalid)"
        await pilot.pause()
