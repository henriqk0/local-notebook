import tempfile
from pathlib import Path

import pytest

from pynotebook import models, repo


@pytest.mark.asyncio
async def test_create_and_list_notebooks():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("Test", "desc")
        assert nb.id is not None
        assert nb.name == "Test"
        nbs = await r.list_notebooks()
        assert len(nbs) == 1
        assert nbs[0].name == "Test"
        await r.close()


@pytest.mark.asyncio
async def test_sources_and_messages():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N1")
        s = models.Source(
            notebook_id=nb.id or 0,
            path="/tmp/file.txt",
            filename="file.txt",
            size=100,
            content_text="hello",
            token_estimate=1,
            enabled=True,
            status="ok",
        )
        s = await r.add_source(s)
        assert s.id is not None
        sources = await r.list_sources(nb.id or 0)
        assert len(sources) == 1
        assert sources[0].filename == "file.txt"

        msg = models.Message(notebook_id=nb.id or 0, role="user", content="hi", model="test/model")
        msg = await r.add_message(msg)
        assert msg.id is not None
        msgs = await r.list_messages(nb.id or 0)
        assert len(msgs) == 1
        await r.close()


@pytest.mark.asyncio
async def test_search_chunks_fts():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N")
        s = models.Source(
            notebook_id=nb.id or 0,
            path="/tmp/x.txt",
            filename="x.txt",
            content_text="the quick brown fox",
            token_estimate=5,
            enabled=True,
            status="ok",
        )
        s = await r.add_source(s)
        await r.add_chunks(s.id or 0, [(0, "the quick brown fox"), (1, "lazy dog sleeps")])
        from pynotebook.llm.context import sanitize_fts_query

        q = sanitize_fts_query("quick fox")
        results = await r.search_chunks(nb.id or 0, q)
        assert len(results) >= 1
        assert results[0][1] == "x.txt"
        await r.close()
