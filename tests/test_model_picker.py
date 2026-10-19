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
            ModelInfo(
                id="openai/gpt-4", name="GPT-4", context_length=8192, is_free=False
            ),
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


class PriceApp(App[None]):
    async def on_mount(self) -> None:
        models = [
            ModelInfo(
                id="paid/model",
                name="Paid",
                context_length=8192,
                prompt_price=5e-7,
                completion_price=1.5e-6,
            ),
            ModelInfo(id="free/model", name="Free", context_length=4096, is_free=True),
        ]
        self.push_screen(ModelPicker(models, current="free/model"))

    def _set(self, result):
        pass


@pytest.mark.asyncio
async def test_model_picker_prices_and_current_marker():
    app = PriceApp()
    async with app.run_test() as pilot:
        picker = app.screen
        from textual.widgets import DataTable

        table = picker.query_one("#model-table", DataTable)
        rows = [table.get_row_at(i) for i in range(table.row_count)]
        paid = next(row for row in rows if "paid/model" in row[0])
        assert paid[2] == "0.50"
        assert paid[3] == "1.50"
        free = next(row for row in rows if "free/model" in row[0])
        assert "→" in free[0]
        await pilot.pause()
