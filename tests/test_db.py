from pathlib import Path

import aiosqlite
import pytest

from pynotebook import db


@pytest.mark.asyncio
async def test_init_db_sets_version_indexes_and_wal(tmp_path: Path):
    conn = await db.init_db(tmp_path / "fresh.db")

    cur = await conn.execute("PRAGMA user_version;")
    row = await cur.fetchone()
    await cur.close()
    assert row is not None and row[0] == db.SCHEMA_VERSION

    cur = await conn.execute("PRAGMA index_list('sources');")
    source_indexes = {r[1] for r in await cur.fetchall()}
    await cur.close()
    assert "idx_sources_notebook" in source_indexes

    cur = await conn.execute("PRAGMA index_list('chunks');")
    chunk_indexes = {r[1] for r in await cur.fetchall()}
    await cur.close()
    assert "idx_chunks_source" in chunk_indexes

    cur = await conn.execute("PRAGMA journal_mode;")
    mode = await cur.fetchone()
    await cur.close()
    assert mode is not None and str(mode[0]).lower() == "wal"

    await conn.close()


@pytest.mark.asyncio
async def test_migration_from_v1_adds_indexes(tmp_path: Path):
    db_path = tmp_path / "old.db"
    conn = await aiosqlite.connect(str(db_path))
    await conn.executescript(db.SCHEMA)
    await conn.execute("PRAGMA user_version = 1;")
    await conn.commit()
    await conn.close()

    conn = await db.init_db(db_path)
    cur = await conn.execute("PRAGMA user_version;")
    row = await cur.fetchone()
    await cur.close()
    assert row is not None and row[0] == db.SCHEMA_VERSION

    cur = await conn.execute("PRAGMA index_list('messages');")
    indexes = {r[1] for r in await cur.fetchall()}
    await cur.close()
    assert "idx_messages_notebook" in indexes
    await conn.close()
