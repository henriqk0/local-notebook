from __future__ import annotations

from ..tokens import CHARS_PER_TOKEN


def chunk_text(
    text: str, chunk_tokens: int = 800, overlap_tokens: int = 100
) -> list[str]:
    if chunk_tokens <= 0:
        raise ValueError("chunk_tokens must be positive")
    if not text:
        return []

    chunk_chars = chunk_tokens * CHARS_PER_TOKEN
    overlap_chars = min(
        max(overlap_tokens, 0) * CHARS_PER_TOKEN,
        chunk_chars // 2,
    )
    chunks: list[str] = []
    current = ""

    def flush() -> None:
        nonlocal current
        if current:
            chunks.append(current)
            current = ""

    for para in (p.strip() for p in text.split("\n\n")):
        if not para:
            continue

        if len(para) > chunk_chars:
            flush()
            step = max(1, chunk_chars - overlap_chars)
            chunks.extend(para[i : i + chunk_chars] for i in range(0, len(para), step))
            continue

        if current and len(current) + len(para) + 2 > chunk_chars:
            previous = current
            flush()
            current = (
                previous[-overlap_chars:] + "\n\n" + para if overlap_chars else para
            )
        else:
            current = f"{current}\n\n{para}" if current else para

    flush()
    return chunks
