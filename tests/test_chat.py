import asyncio
from typing import ClassVar

import pytest
from textual.app import App

from pynotebook import config, models, repo
from pynotebook.llm import openrouter
from pynotebook.ui.screens import NotebookScreen


class FakeClient:
    """Replaces OpenRouterClient in tests."""

    behavior = "ok"
    tokens: ClassVar[list[str]] = ["Hel", "lo", " world"]

    def __init__(self, api_key, client=None):
        self.api_key = api_key
        self.last_messages = None

    async def aclose(self):
        pass

    async def stream_chat(self, model, messages):
        FakeClient.last_messages = messages
        if FakeClient.behavior == "error":
            raise openrouter.OpenRouterError("Rate limited (429).", 429)
        if FakeClient.behavior == "slow":
            for tok in self.tokens:
                yield tok
                await asyncio.sleep(0.05)
            return
        for tok in self.tokens:
            yield tok


class ChatApp(App[None]):
    def __init__(self, repository):
        super().__init__()
        self.repo = repository
        self.cfg = config.Config(api_key="testkey", default_model="test/model")

    async def on_mount(self):
        await self.push_screen(NotebookScreen(1, "Chat NB"))


async def _setup(tmp_path):
    r = await repo.Repository.from_path(tmp_path / "t.db")
    nb = await r.create_notebook("N1")
    s = models.Source(
        notebook_id=nb.id or 0,
        path="/tmp/a.txt",
        filename="a.txt",
        content_text="The answer is 42.",
        token_estimate=5,
        enabled=True,
        status="ok",
    )
    s = await r.add_source(s)
    await r.add_chunks(s.id or 0, [(0, "The answer is 42.")])
    return r, nb


@pytest.fixture(autouse=True)
def _patch_client(monkeypatch):
    monkeypatch.setattr(openrouter, "OpenRouterClient", FakeClient)
    FakeClient.behavior = "ok"


@pytest.mark.asyncio
async def test_streaming_persists_and_renders(tmp_path):
    r, nb = await _setup(tmp_path)
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        await screen._generate("What is the answer?", [])
        await pilot.pause()
        msgs = await r.list_messages(nb.id or 0)
        roles = [m.role for m in msgs]
        assert roles == ["assistant"]
        assert "Hello world" in msgs[-1].content
    await r.close()


@pytest.mark.asyncio
async def test_error_is_recorded(tmp_path):
    r, nb = await _setup(tmp_path)
    FakeClient.behavior = "error"
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        await screen._generate("q", [])
        await pilot.pause()
        msgs = await r.list_messages(nb.id or 0)
        assert any(m.role == "error" for m in msgs)
        assert any("Rate limited" in m.content for m in msgs)
    await r.close()


@pytest.mark.asyncio
async def test_cancel_persists_partial(tmp_path):
    r, nb = await _setup(tmp_path)
    FakeClient.behavior = "slow"
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        task = asyncio.create_task(screen._generate("q", []))
        await asyncio.sleep(0.08)  # let a couple tokens through
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        await pilot.pause()
        msgs = await r.list_messages(nb.id or 0)
        assert any("interrupted" in m.content for m in msgs)
    await r.close()


@pytest.mark.asyncio
async def test_full_submit_flow(tmp_path):
    r, nb = await _setup(tmp_path)
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        await screen._start_generation("Hello there")
        # wait for worker
        while screen._gen_worker is not None:
            await pilot.pause()
            await asyncio.sleep(0.01)
        msgs = await r.list_messages(nb.id or 0)
        roles = [m.role for m in msgs]
        assert roles == ["user", "assistant"]
        assert msgs[0].content == "Hello there"
    await r.close()


@pytest.mark.asyncio
async def test_typing_in_input_does_not_trigger_source_keys(tmp_path):
    r, nb = await _setup(tmp_path)
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        from pynotebook.ui.widgets.chat import ChatInput

        inp = screen.query_one("#chat-input", ChatInput)
        inp.focus()
        await pilot.pause()
        await pilot.press("space")
        await pilot.press("a")
        await pilot.press("x")
        await pilot.pause()
        assert inp.text == " ax"
        # source should still be enabled (space/x did not toggle/remove)
        sources = await r.list_sources(nb.id or 0)
        assert len(sources) == 1
        assert sources[0].enabled is True
    await r.close()


@pytest.mark.asyncio
async def test_sources_table_space_toggles(tmp_path):
    r, nb = await _setup(tmp_path)
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        table = screen.query_one("#sources")
        table.focus()
        await pilot.pause()
        await pilot.press("space")
        await pilot.pause()
        sources = await r.list_sources(nb.id or 0)
        assert sources[0].enabled is False
        await pilot.press("space")
        await pilot.pause()
        sources = await r.list_sources(nb.id or 0)
        assert sources[0].enabled is True
    await r.close()


@pytest.mark.asyncio
async def test_add_source_falls_back_to_path_dialog(tmp_path):
    r, _nb = await _setup(tmp_path)
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        table = screen.query_one("#sources")
        table.focus()
        await pilot.pause()
        await pilot.press("a")
        await pilot.pause()
        from pynotebook.ui.modals import PathInputDialog

        assert isinstance(app.screen, PathInputDialog)
        await pilot.press("escape")
        await pilot.pause()
    await r.close()


@pytest.mark.asyncio
async def test_double_submit_is_ignored(tmp_path):
    r, nb = await _setup(tmp_path)
    FakeClient.behavior = "slow"
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        await screen._start_generation("first")
        await screen._start_generation("second")
        while screen._gen_worker is not None:
            await pilot.pause()
            await asyncio.sleep(0.01)
        msgs = await r.list_messages(nb.id or 0)
        user_messages = [m.content for m in msgs if m.role == "user"]
        assert user_messages == ["first"]
    await r.close()


@pytest.mark.asyncio
async def test_clear_chat_cancels_generation_and_clears(tmp_path):
    r, nb = await _setup(tmp_path)
    FakeClient.behavior = "slow"
    app = ChatApp(r)
    async with app.run_test() as pilot:
        screen = app.screen
        await screen._start_generation("hello")
        await asyncio.sleep(0.08)
        await screen._clear_chat()
        await pilot.pause()
        msgs = await r.list_messages(nb.id or 0)
        assert msgs == []
    await r.close()


def test_safe_text_strips_control_characters():
    from pynotebook.ui.screens.notebook import _safe_text

    assert _safe_text("a\x00b\x1bc\nd\te") == "abc\nd\te"
