from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass

from .. import models

CHARS_PER_TOKEN = 4


def estimate_tokens(text: str) -> int:
    return len(text) // CHARS_PER_TOKEN if text else 0


def sanitize_fts_query(query: str) -> str:
    """Turn a free-text query into a safe FTS5 MATCH expression.

    Quotes each alphanumeric token and ORs them together so partial matches
    still return results without syntax errors.
    """
    tokens = re.findall(r"[A-Za-z0-9_]+", query)
    if not tokens:
        return ""
    return " OR ".join(f'"{t}"' for t in tokens)


@dataclass
class ChunkHit:
    source_id: int
    filename: str
    text: str


@dataclass
class ContextResult:
    system_prompt: str
    sources_block: str
    history: list[models.Message]
    used_tokens: int
    mode: str  # "full" or "retrieval"


def _render_source(filename: str, text: str) -> str:
    return f"=== SOURCE: {filename} ===\n{text}\n=== END SOURCE: {filename} ==="


def build_sources_block_full(sources: list[models.Source]) -> str:
    parts = [_render_source(s.filename, s.content_text) for s in sources if s.content_text]
    return "\n\n".join(parts)


def build_sources_block_retrieval(
    hits: list[ChunkHit],
    token_budget: int,
) -> str:
    # Group per source, preserving first-seen order.
    grouped: dict[int, list[ChunkHit]] = {}
    order: list[int] = []
    for h in hits:
        if h.source_id not in grouped:
            grouped[h.source_id] = []
            order.append(h.source_id)
        grouped[h.source_id].append(h)

    blocks: list[str] = []
    used = 0
    for source_id in order:
        filename = grouped[source_id][0].filename
        pieces: list[str] = []
        for h in grouped[source_id]:
            cost = estimate_tokens(h.text)
            if used + cost > token_budget:
                break
            pieces.append(h.text)
            used += cost
        if pieces:
            blocks.append(_render_source(filename, "\n\n".join(pieces)))
        if used >= token_budget:
            break
    return "\n\n".join(blocks)


def trim_history(
    history: list[models.Message],
    token_budget: int,
) -> list[models.Message]:
    """Keep newest turns that fit the budget, dropping oldest first."""
    kept: list[models.Message] = []
    used = 0
    for msg in reversed(history):
        cost = estimate_tokens(msg.content)
        if used + cost > token_budget and kept:
            break
        kept.append(msg)
        used += cost
    kept.reverse()
    return kept


def build_context(
    question: str,
    sources: list[models.Source],
    history: list[models.Message],
    system_prompt: str,
    token_budget: int,
    search_chunks: Callable[[str], list[ChunkHit]] | None = None,
) -> ContextResult:
    """Build the prompt pieces for a chat request.

    If all enabled sources fit the budget, uses their full text. Otherwise
    falls back to BM25 retrieval via ``search_chunks`` (if provided).
    """
    enabled = [s for s in sources if s.enabled and s.status == "ok"]
    total_source_tokens = sum(estimate_tokens(s.content_text) for s in enabled)

    # Reserve some budget for history and the answer.
    history_budget = max(token_budget // 4, 500)

    if total_source_tokens <= token_budget:
        sources_block = build_sources_block_full(enabled)
        mode = "full"
    else:
        hits: list[ChunkHit] = search_chunks(question) if search_chunks else []
        sources_block = build_sources_block_retrieval(hits, token_budget)
        mode = "retrieval"

    trimmed = trim_history(history, history_budget)
    used_tokens = (
        estimate_tokens(system_prompt)
        + estimate_tokens(sources_block)
        + sum(estimate_tokens(m.content) for m in trimmed)
    )
    return ContextResult(
        system_prompt=system_prompt,
        sources_block=sources_block,
        history=trimmed,
        used_tokens=used_tokens,
        mode=mode,
    )
