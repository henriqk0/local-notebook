from __future__ import annotations

from pathlib import Path

from .. import models
from . import chunk, extract


def build_source_from_path(notebook_id: int, path: Path) -> tuple[models.Source, list[str]]:
    path = path.expanduser()
    try:
        resolved = path.resolve()
    except OSError:
        resolved = path

    filename = resolved.name
    size = 0
    try:
        size = resolved.stat().st_size
    except OSError:
        size = 0

    content, size, ext, mime, err = extract.extract_text(resolved)

    source = models.Source(
        notebook_id=notebook_id,
        path=str(resolved),
        filename=filename,
        mime=mime,
        ext=ext,
        size=size,
        content_text=content,
        token_estimate=extract.estimate_tokens(content),
        enabled=err is None,
        status="ok" if err is None else "error",
        error=err,
    )

    chunks: list[str] = []
    if err is None and content:
        chunks = chunk.chunk_text(content)
    return source, chunks
