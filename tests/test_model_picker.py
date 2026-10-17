import pytest
from textual.app import App

from pynotebook.llm.models_cache import ModelInfo
from pynotebook.ui.modals import ModelPicker


class DummyApp(App[None]):
    def __init__(self):
        super().__init__()
        self.result = None

    async def on_mount(self) -> None:
        models = [
            ModelInfo(id="openai/gpt-4", name="GPT-4", context_length=8192, is_free=False),
            ModelInfo(id="meta/llama", name="Llama", context_length=4096, is_free=True),
        ]
        self.push_screen(ModelPicker(models, current="meta/llama"), callback=self._set)

    def _set(self, result):
        self.result = result


@pytest.mark.asyncio
async def test_model_picker_mount_and_filter():
    app = DummyApp()
    async with app.run_test() as pilot:
        picker = app.screen
        assert isinstance(picker, ModelPicker)
        # filter
        from textual.widgets import Input

        inp = picker.query_one("#model-search", Input)
        inp.value = "llama"
        await pilot.pause()
        assert len(picker._filtered) == 1
        assert picker._filtered[0].id == "meta/llama"
