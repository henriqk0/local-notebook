from __future__ import annotations

import os
from pathlib import Path

import aiosqlite

from .config import ensure_private_dir

SCHEMA_VERSION = 2

SCHEMA = """
CREATE TABLE IF NOT EXISTS notebooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT DEFAULT '',
    model TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    notebook_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime TEXT,
    ext TEXT,
    size INTEGER DEFAULT 0,
    content_text TEXT DEFAULT '',
    token_estimate INTEGER DEFAULT 0,
    enabled INTEGER DEFAULT 1,
    status TEXT DEFAULT 'ok',
    error TEXT,
    added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(notebook_id) REFERENCES notebooks(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    text TEXT NOT NULL,
    FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    notebook_id INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    model TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(notebook_id) REFERENCES notebooks(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text,
    content='chunks',
    content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
"""

MIGRATION_2 = """
CREATE INDEX IF NOT EXISTS idx_sources_notebook ON sources(notebook_id);
CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id);
CREATE INDEX IF NOT EXISTS idx_messages_notebook ON messages(notebook_id);
"""


async def init_db(db_path: Path) -> aiosqlite.Connection:
    ensure_private_dir(db_path.parent)
    conn = await aiosqlite.connect(str(db_path))
    await conn.execute("PRAGMA foreign_keys = ON;")
    await _execute_pragma(conn, "PRAGMA journal_mode = WAL;")
    await _execute_pragma(conn, "PRAGMA synchronous = NORMAL;")
    await _execute_pragma(conn, "PRAGMA busy_timeout = 5000;")
    await run_migrations(conn)
    try:
        os.chmod(db_path, 0o600)
    except OSError:
        pass
    return conn


async def _execute_pragma(conn: aiosqlite.Connection, pragma: str) -> None:
    cur = await conn.execute(pragma)
    await cur.close()


async def run_migrations(conn: aiosqlite.Connection) -> None:
    cur = await conn.execute("PRAGMA user_version;")
    row = await cur.fetchone()
    version = int(row[0]) if row else 0
    await cur.close()

    current_version = version
    if version < 1:
        await conn.executescript(SCHEMA)
        version = 1
    if version < 2:
        await conn.executescript(MIGRATION_2)
        version = 2

    if version != current_version:
        await conn.execute(f"PRAGMA user_version = {version};")
    await conn.commit()
