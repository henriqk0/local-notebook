import asyncio
from pathlib import Path

import pytest
from textual.widgets import DataTable

from pynotebook.app import PyNotebookApp


@pytest.mark.asyncio
async def test_filtered_delete_targets_visible_row(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-or-test-not-real")

    app = PyNotebookApp()
    async with app.run_test() as pilot:
        repository = app.repo
        assert repository is not None
        await repository.create_notebook("alpha")
        await repository.create_notebook("beta")
        await repository.create_notebook("gamma")
        await app._refresh_notebook_list(app.screen)

        app._filter_text = "beta"
        await app._refresh_notebook_list(app.screen)
        table = app.screen.query_one(DataTable)
        assert table.row_count == 1

        captured: dict[str, object] = {}

        def fake_push_screen(screen, callback=None, **kwargs):
            captured["callback"] = callback

        monkeypatch.setattr(app, "push_screen", fake_push_screen)
        await app.action_delete_notebook()
        callback = captured["callback"]
        assert callback is not None
        callback(True)  # type: ignore[operator]

        for _ in range(200):
            await pilot.pause()
            names = [nb.name for nb in await repository.list_notebooks()]
            if "beta" not in names:
                break
            await asyncio.sleep(0.01)

        names = [nb.name for nb in await repository.list_notebooks()]
        assert sorted(names) == ["alpha", "gamma"]
