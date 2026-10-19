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

        msg = models.Message(
            notebook_id=nb.id or 0, role="user", content="hi", model="test/model"
        )
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
        await r.add_chunks(
            s.id or 0, [(0, "the quick brown fox"), (1, "lazy dog sleeps")]
        )
        from pynotebook.llm.context import sanitize_fts_query

        q = sanitize_fts_query("quick fox")
        results = await r.search_chunks(nb.id or 0, q)
        assert len(results) >= 1
        assert results[0][1] == "x.txt"
        await r.close()


@pytest.mark.asyncio
async def test_notebook_counts_and_source_meta():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb1 = await r.create_notebook("N1")
        await r.create_notebook("N2")
        source = models.Source(
            notebook_id=nb1.id or 0,
            path="/tmp/a.txt",
            filename="a.txt",
            content_text="full content",
            token_estimate=2,
            enabled=True,
            status="ok",
        )
        await r.add_source(source)

        counts = {nb.name: count for nb, count in await r.list_notebooks_with_counts()}
        assert counts == {"N1": 1, "N2": 0}

        meta = await r.list_source_meta(nb1.id or 0)
        assert len(meta) == 1
        assert meta[0].content_text == ""
        assert meta[0].token_estimate == 2

        full = await r.list_sources(nb1.id or 0)
        assert full[0].content_text == "full content"
        await r.close()


@pytest.mark.asyncio
async def test_add_source_with_chunks_is_atomic():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N")
        source = models.Source(
            notebook_id=nb.id or 0,
            path="/tmp/a.txt",
            filename="a.txt",
            content_text="hello",
            token_estimate=1,
            enabled=True,
            status="ok",
        )
        with pytest.raises(ValueError):
            await r.add_source_with_chunks(source, [(0,)])  # type: ignore[list-item]

        assert await r.list_sources(nb.id or 0) == []
        cur = await r.conn.execute("SELECT COUNT(*) FROM chunks;")
        row = await cur.fetchone()
        await cur.close()
        assert row is not None and row[0] == 0
        await r.close()


@pytest.mark.asyncio
async def test_replace_source_content_swaps_chunks_and_status():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N")
        source = models.Source(
            notebook_id=nb.id or 0,
            path="/tmp/a.txt",
            filename="a.txt",
            content_text="old text",
            token_estimate=2,
            enabled=True,
            status="ok",
        )
        source = await r.add_source_with_chunks(source, [(0, "old text")])

        updated = await r.replace_source_content(
            source.id or 0,
            content_text="new text",
            token_estimate=2,
            size=8,
            mime="text/plain",
            ext="txt",
            filename="a.txt",
            status="ok",
            error=None,
            chunks=[(0, "new text")],
        )
        assert updated is not None
        assert updated.content_text == "new text"
        assert updated.enabled is True

        cur = await r.conn.execute(
            "SELECT text FROM chunks WHERE source_id = ? ORDER BY idx;", (source.id,)
        )
        texts = [row[0] for row in await cur.fetchall()]
        await cur.close()
        assert texts == ["new text"]

        broken = await r.replace_source_content(
            source.id or 0,
            content_text="",
            token_estimate=0,
            size=0,
            mime=None,
            ext="txt",
            filename="a.txt",
            status="error",
            error="broken",
            chunks=[],
        )
        assert broken is not None
        assert broken.enabled is False
        assert broken.status == "error"
        await r.close()


@pytest.mark.asyncio
async def test_list_messages_limit_returns_newest_in_order():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N")
        for i in range(5):
            await r.add_message(
                models.Message(notebook_id=nb.id or 0, role="user", content=str(i))
            )
        messages = await r.list_messages(nb.id or 0, limit=2)
        assert [m.content for m in messages] == ["3", "4"]
        await r.close()


@pytest.mark.asyncio
async def test_mark_source_error_keeps_content():
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "test.db"
        r = await repo.Repository.from_path(db_path)
        nb = await r.create_notebook("N")
        source = models.Source(
            notebook_id=nb.id or 0,
            path="/tmp/a.txt",
            filename="a.txt",
            content_text="keep me",
            token_estimate=2,
            enabled=True,
            status="ok",
        )
        source = await r.add_source_with_chunks(source, [(0, "keep me")])
        broken = await r.mark_source_error(source.id or 0, "File no longer exists")
        assert broken is not None
        assert broken.enabled is False
        assert broken.status == "error"
        assert broken.content_text == "keep me"
        await r.close()
