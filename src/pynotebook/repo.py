from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import Any

import aiosqlite

from . import db, models


def _row_to_notebook(row: tuple[Any, ...]) -> models.Notebook:
    return models.Notebook(
        id=row[0],
        name=row[1],
        description=row[2] or "",
        model=row[3],
        created_at=datetime.fromisoformat(row[4]) if row[4] else None,
        updated_at=datetime.fromisoformat(row[5]) if row[5] else None,
    )


def _row_to_source(row: tuple[Any, ...]) -> models.Source:
    return models.Source(
        id=row[0],
        notebook_id=row[1],
        path=row[2],
        filename=row[3],
        mime=row[4],
        ext=row[5],
        size=row[6] or 0,
        content_text=row[7] or "",
        token_estimate=row[8] or 0,
        enabled=bool(row[9]) if row[9] is not None else True,
        status=row[10] or "ok",
        error=row[11],
        added_at=datetime.fromisoformat(row[12]) if row[12] else None,
    )


def _row_to_message(row: tuple[Any, ...]) -> models.Message:
    return models.Message(
        id=row[0],
        notebook_id=row[1],
        role=row[2],
        content=row[3],
        model=row[4],
        created_at=datetime.fromisoformat(row[5]) if row[5] else None,
    )


SOURCE_COLUMNS = (
    "id, notebook_id, path, filename, mime, ext, size, content_text, "
    "token_estimate, enabled, status, error, added_at"
)

SOURCE_META_COLUMNS = (
    "id, notebook_id, path, filename, mime, ext, size, '' AS content_text, "
    "token_estimate, enabled, status, error, added_at"
)


class Repository:
    def __init__(self, conn: aiosqlite.Connection) -> None:
        self.conn = conn

    @classmethod
    async def from_path(cls, db_path: Path) -> Repository:
        conn = await db.init_db(db_path)
        return cls(conn)

    async def close(self) -> None:
        await self.conn.close()

    # Notebooks
    async def create_notebook(
        self, name: str, description: str = ""
    ) -> models.Notebook:
        cur = await self.conn.execute(
            "INSERT INTO notebooks (name, description) VALUES (?, ?)",
            (name, description),
        )
        notebook_id = cur.lastrowid
        await self.conn.commit()
        if notebook_id is None:  # pragma: no cover - SQLite always returns it
            notebook_id = 0
        return models.Notebook(id=notebook_id, name=name, description=description)

    async def list_notebooks(self) -> list[models.Notebook]:
        cur = await self.conn.execute(
            "SELECT id, name, description, model, created_at, updated_at FROM notebooks ORDER BY updated_at DESC, id DESC"
        )
        rows = await cur.fetchall()
        await cur.close()
        return [_row_to_notebook(r) for r in rows]

    async def list_notebooks_with_counts(self) -> list[tuple[models.Notebook, int]]:
        cur = await self.conn.execute(
            """
            SELECT n.id, n.name, n.description, n.model, n.created_at, n.updated_at,
                   COUNT(s.id)
            FROM notebooks AS n
            LEFT JOIN sources AS s ON s.notebook_id = n.id
            GROUP BY n.id
            ORDER BY n.updated_at DESC, n.id DESC
            """
        )
        rows = await cur.fetchall()
        await cur.close()
        return [(_row_to_notebook(r[:6]), int(r[6])) for r in rows]

    async def get_notebook(self, notebook_id: int) -> models.Notebook | None:
        cur = await self.conn.execute(
            "SELECT id, name, description, model, created_at, updated_at FROM notebooks WHERE id = ?",
            (notebook_id,),
        )
        row = await cur.fetchone()
        await cur.close()
        return _row_to_notebook(row) if row else None

    async def update_notebook(
        self,
        notebook_id: int,
        name: str | None = None,
        description: str | None = None,
        model: str | None = None,
    ) -> models.Notebook | None:
        nb = await self.get_notebook(notebook_id)
        if not nb:
            return None
        if name is not None:
            nb.name = name
        if description is not None:
            nb.description = description
        if model is not None:
            nb.model = model
        await self.conn.execute(
            "UPDATE notebooks SET name = ?, description = ?, model = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            (nb.name, nb.description, nb.model, notebook_id),
        )
        await self.conn.commit()
        return await self.get_notebook(notebook_id)

    async def delete_notebook(self, notebook_id: int) -> bool:
        cur = await self.conn.execute(
            "DELETE FROM notebooks WHERE id = ?", (notebook_id,)
        )
        changed = cur.rowcount
        await cur.close()
        await self.conn.commit()
        return changed > 0

    # Sources
    async def add_source(self, source: models.Source) -> models.Source:
        await self.conn.execute(
            """INSERT INTO sources (notebook_id, path, filename, mime, ext, size, content_text, token_estimate, enabled, status, error)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                source.notebook_id,
                source.path,
                source.filename,
                source.mime,
                source.ext,
                source.size,
                source.content_text,
                source.token_estimate,
                1 if source.enabled else 0,
                source.status,
                source.error,
            ),
        )
        cur = await self.conn.execute("SELECT last_insert_rowid();")
        row = await cur.fetchone()
        source.id = row[0] if row else source.id
        await cur.close()
        await self.conn.commit()
        return source

    async def add_source_with_chunks(
        self,
        source: models.Source,
        chunks: list[tuple[int, str]],
    ) -> models.Source:
        try:
            cur = await self.conn.execute(
                """INSERT INTO sources (notebook_id, path, filename, mime, ext, size, content_text, token_estimate, enabled, status, error)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    source.notebook_id,
                    source.path,
                    source.filename,
                    source.mime,
                    source.ext,
                    source.size,
                    source.content_text,
                    source.token_estimate,
                    1 if source.enabled else 0,
                    source.status,
                    source.error,
                ),
            )
            source.id = cur.lastrowid
            if source.id is not None and chunks:
                await self.conn.executemany(
                    "INSERT INTO chunks (source_id, idx, text) VALUES (?, ?, ?)",
                    [(source.id, idx, text) for idx, text in chunks],
                )
            await self.conn.commit()
        except BaseException:
            await self.conn.rollback()
            raise
        return source

    async def list_sources(self, notebook_id: int) -> list[models.Source]:
        cur = await self.conn.execute(
            f"""SELECT {SOURCE_COLUMNS}
               FROM sources WHERE notebook_id = ? ORDER BY id ASC""",
            (notebook_id,),
        )
        rows = await cur.fetchall()
        await cur.close()
        return [_row_to_source(r) for r in rows]

    async def list_source_meta(self, notebook_id: int) -> list[models.Source]:
        """List source metadata without loading full content into memory."""
        cur = await self.conn.execute(
            f"""SELECT {SOURCE_META_COLUMNS}
               FROM sources WHERE notebook_id = ? ORDER BY id ASC""",
            (notebook_id,),
        )
        rows = await cur.fetchall()
        await cur.close()
        return [_row_to_source(r) for r in rows]

    async def list_sources_for_context(self, notebook_id: int) -> list[models.Source]:
        """Load only enabled, healthy sources for prompt building."""
        cur = await self.conn.execute(
            f"""SELECT {SOURCE_COLUMNS}
               FROM sources
               WHERE notebook_id = ? AND enabled = 1 AND status = 'ok'
               ORDER BY id ASC""",
            (notebook_id,),
        )
        rows = await cur.fetchall()
        await cur.close()
        return [_row_to_source(r) for r in rows]

    async def get_source(self, source_id: int) -> models.Source | None:
        cur = await self.conn.execute(
            f"""SELECT {SOURCE_COLUMNS}
               FROM sources WHERE id = ?""",
            (source_id,),
        )
        row = await cur.fetchone()
        await cur.close()
        return _row_to_source(row) if row else None

    async def toggle_source(
        self, source_id: int, enabled: bool
    ) -> models.Source | None:
        await self.conn.execute(
            "UPDATE sources SET enabled = ? WHERE id = ?",
            (1 if enabled else 0, source_id),
        )
        await self.conn.commit()
        return await self.get_source(source_id)

    async def delete_source(self, source_id: int) -> bool:
        cur = await self.conn.execute("DELETE FROM sources WHERE id = ?", (source_id,))
        changed = cur.rowcount
        await cur.close()
        await self.conn.commit()
        return changed > 0

    async def update_source_content(
        self,
        source_id: int,
        content_text: str,
        token_estimate: int,
        status: str = "ok",
        error: str | None = None,
    ) -> models.Source | None:
        await self.conn.execute(
            "UPDATE sources SET content_text = ?, token_estimate = ?, status = ?, error = ? WHERE id = ?",
            (content_text, token_estimate, status, error, source_id),
        )
        await self.conn.commit()
        return await self.get_source(source_id)

    async def mark_source_error(
        self, source_id: int, error: str
    ) -> models.Source | None:
        """Mark a source as broken without discarding its existing content."""
        await self.conn.execute(
            "UPDATE sources SET status = 'error', error = ?, enabled = 0 WHERE id = ?",
            (error, source_id),
        )
        await self.conn.commit()
        return await self.get_source(source_id)

    async def replace_source_content(
        self,
        source_id: int,
        *,
        content_text: str,
        token_estimate: int,
        size: int,
        mime: str | None,
        ext: str | None,
        filename: str,
        status: str,
        error: str | None,
        chunks: list[tuple[int, str]],
    ) -> models.Source | None:
        """Atomically replace source text, metadata, and chunks."""
        try:
            await self.conn.execute(
                """UPDATE sources
                   SET content_text = ?, token_estimate = ?, size = ?, mime = ?,
                       ext = ?, filename = ?, status = ?, error = ?,
                       enabled = CASE WHEN ? = 'ok' THEN 1 ELSE 0 END
                   WHERE id = ?""",
                (
                    content_text,
                    token_estimate,
                    size,
                    mime,
                    ext,
                    filename,
                    status,
                    error,
                    status,
                    source_id,
                ),
            )
            await self.conn.execute(
                "DELETE FROM chunks WHERE source_id = ?", (source_id,)
            )
            if chunks:
                await self.conn.executemany(
                    "INSERT INTO chunks (source_id, idx, text) VALUES (?, ?, ?)",
                    [(source_id, idx, text) for idx, text in chunks],
                )
            await self.conn.commit()
        except BaseException:
            await self.conn.rollback()
            raise
        return await self.get_source(source_id)

    # Chunks
    async def add_chunks(self, source_id: int, chunks: list[tuple[int, str]]) -> None:
        if not chunks:
            return
        await self.conn.executemany(
            "INSERT INTO chunks (source_id, idx, text) VALUES (?, ?, ?)",
            [(source_id, idx, text) for idx, text in chunks],
        )
        await self.conn.commit()

    async def clear_chunks(self, source_id: int) -> None:
        await self.conn.execute("DELETE FROM chunks WHERE source_id = ?", (source_id,))
        await self.conn.commit()

    # Messages
    async def add_message(self, message: models.Message) -> models.Message:
        cur = await self.conn.execute(
            "INSERT INTO messages (notebook_id, role, content, model) VALUES (?, ?, ?, ?)",
            (message.notebook_id, message.role, message.content, message.model),
        )
        message.id = cur.lastrowid
        await self.conn.commit()
        return message

    async def list_messages(
        self, notebook_id: int, limit: int | None = None
    ) -> list[models.Message]:
        if limit is None:
            cur = await self.conn.execute(
                "SELECT id, notebook_id, role, content, model, created_at FROM messages WHERE notebook_id = ? ORDER BY id ASC",
                (notebook_id,),
            )
        else:
            cur = await self.conn.execute(
                "SELECT id, notebook_id, role, content, model, created_at FROM messages WHERE notebook_id = ? ORDER BY id DESC LIMIT ?",
                (notebook_id, max(limit, 1)),
            )
        rows = await cur.fetchall()
        await cur.close()
        messages = [_row_to_message(r) for r in rows]
        if limit is not None:
            messages.reverse()
        return messages

    async def search_chunks(
        self, notebook_id: int, match_query: str, limit: int = 20
    ) -> list[tuple[int, str, str]]:
        """BM25-ranked chunk search within a notebook.

        Returns (source_id, filename, text) tuples ordered by relevance.
        """
        if not match_query:
            return []
        cur = await self.conn.execute(
            """
            SELECT c.source_id, s.filename, c.text
            FROM chunks_fts
            JOIN chunks c ON c.id = chunks_fts.rowid
            JOIN sources s ON s.id = c.source_id
            WHERE chunks_fts MATCH ?
              AND s.notebook_id = ?
              AND s.enabled = 1
              AND s.status = 'ok'
            ORDER BY bm25(chunks_fts)
            LIMIT ?
            """,
            (match_query, notebook_id, max(limit, 1)),
        )
        rows = await cur.fetchall()
        await cur.close()
        return [(r[0], r[1], r[2]) for r in rows]

    async def clear_messages(self, notebook_id: int) -> bool:
        cur = await self.conn.execute(
            "DELETE FROM messages WHERE notebook_id = ?", (notebook_id,)
        )
        changed = cur.rowcount
        await cur.close()
        await self.conn.commit()
        return changed > 0
