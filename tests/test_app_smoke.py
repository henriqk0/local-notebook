from pathlib import Path

import pytest

from pynotebook.app import PyNotebookApp


@pytest.mark.asyncio
async def test_app_mounts_with_real_stylesheet(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-or-test-not-real")

    app = PyNotebookApp()
    async with app.run_test() as pilot:
        assert app.screen.__class__.__name__ == "NotebookListScreen"
        await pilot.press("n")
        await pilot.pause()
        assert app.screen.__class__.__name__ == "NotebookForm"
        await pilot.press("escape")
        await pilot.pause()
        assert app.screen.__class__.__name__ == "NotebookListScreen"

    # The shutdown hook must close the non-daemon aiosqlite worker thread.
    assert app.repo is None
