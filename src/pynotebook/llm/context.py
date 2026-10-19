from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, replace

from .. import models
from ..tokens import CHARS_PER_TOKEN, estimate_tokens

OVERHEAD_TOKENS = 64
MIN_HISTORY_TOKENS = 256
MAX_FTS_TOKENS = 32


def sanitize_fts_query(query: str) -> str:
    """Turn a free-text query into a safe FTS5 MATCH expression.

    Tokens are quoted and lowercased, deduplicated, and capped so a pasted
    wall of text cannot build an unbounded MATCH expression. A trailing ``*``
    enables prefix matching (``notebook*`` matches ``notebooks``).
    """
    tokens = re.findall(r"\w+", query, flags=re.UNICODE)
    unique: list[str] = []
    seen: set[str] = set()
    for token in tokens:
        lowered = token.lower()
        if lowered in seen:
            continue
        seen.add(lowered)
        unique.append(lowered)
        if len(unique) >= MAX_FTS_TOKENS:
            break
    return " OR ".join(f'"{t}"*' for t in unique)


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
    parts = [
        _render_source(s.filename, s.content_text) for s in sources if s.content_text
    ]
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
    remaining = token_budget

    for source_id in order:
        filename = grouped[source_id][0].filename
        marker_cost = estimate_tokens(_render_source(filename, "")) + 2
        if remaining <= marker_cost:
            break
        remaining -= marker_cost

        pieces: list[str] = []
        for h in grouped[source_id]:
            cost = estimate_tokens(h.text) + 8
            if cost > remaining:
                # Skip a chunk that doesn't fit; a later, smaller chunk may.
                continue
            pieces.append(h.text)
            remaining -= cost
            if remaining <= 0:
                break
        if pieces:
            blocks.append(_render_source(filename, "\n\n".join(pieces)))
        if remaining <= 0:
            break

    return "\n\n".join(blocks)


def allocate_budgets(
    system_prompt: str,
    question: str,
    token_budget: int,
    answer_reserve: int = 1024,
) -> tuple[int, int, int]:
    """Return (fixed_tokens, history_budget, source_budget)."""
    reserve = min(max(answer_reserve, 0), max(token_budget // 2, 0))
    overhead = min(OVERHEAD_TOKENS, max(token_budget // 8, 0))
    fixed_tokens = estimate_tokens(system_prompt) + estimate_tokens(question) + overhead
    available = max(token_budget - reserve - fixed_tokens, 0)
    if available >= MIN_HISTORY_TOKENS * 2:
        history_budget = min(max(available // 3, MIN_HISTORY_TOKENS), available)
    else:
        # Tiny budgets should prioritize source text over history.
        history_budget = 0
    source_budget = max(available - history_budget, 0)
    return fixed_tokens, history_budget, source_budget


def _truncate(text: str, max_tokens: int) -> str:
    if max_tokens <= 0:
        return ""
    return text[: max_tokens * CHARS_PER_TOKEN]


def trim_history(
    history: list[models.Message],
    token_budget: int,
) -> list[models.Message]:
    """Keep newest complete turns that fit the budget, dropping oldest first."""
    if token_budget <= 0:
        return []

    turns: list[list[models.Message]] = []
    for msg in history:
        if msg.role not in ("user", "assistant"):
            continue
        if msg.role == "user" or not turns:
            turns.append([msg])
        else:
            turns[-1].append(msg)

    kept: list[list[models.Message]] = []
    used = 0

    for turn in reversed(turns):
        cost = sum(estimate_tokens(m.content) for m in turn)
        if used + cost <= token_budget:
            kept.append(turn)
            used += cost
            continue

        if not kept:
            newest_user = next((m for m in turn if m.role == "user"), None)
            assistants = [m for m in turn if m.role == "assistant"]
            assistant_cost = sum(estimate_tokens(m.content) for m in assistants)
            if newest_user is not None and assistant_cost <= token_budget:
                user_budget = max(token_budget - assistant_cost, 0)
                kept_turn: list[models.Message] = []
                if user_budget > 0:
                    kept_turn.append(
                        replace(
                            newest_user,
                            content=_truncate(newest_user.content, user_budget),
                        )
                    )
                kept_turn.extend(assistants)
                if kept_turn:
                    kept.append(kept_turn)
            elif turn:
                last = turn[-1]
                kept.append(
                    [replace(last, content=_truncate(last.content, token_budget))]
                )
        break

    return [msg for turn in reversed(kept) for msg in turn]


def build_context(
    question: str,
    sources: list[models.Source],
    history: list[models.Message],
    system_prompt: str,
    token_budget: int,
    search_chunks: Callable[[str], list[ChunkHit]] | None = None,
    answer_reserve: int = 1024,
) -> ContextResult:
    """Build the prompt pieces for a chat request.

    If all enabled sources fit the source budget, uses their full text.
    Otherwise falls back to BM25 retrieval via ``search_chunks`` (if provided).
    """
    enabled = [s for s in sources if s.enabled and s.status == "ok"]
    total_source_tokens = sum(estimate_tokens(s.content_text) for s in enabled)

    fixed_tokens, history_budget, source_budget = allocate_budgets(
        system_prompt, question, token_budget, answer_reserve
    )

    if total_source_tokens <= source_budget:
        sources_block = build_sources_block_full(enabled)
        mode = "full"
    else:
        hits: list[ChunkHit] = search_chunks(question) if search_chunks else []
        sources_block = build_sources_block_retrieval(hits, source_budget)
        mode = "retrieval"

    trimmed = trim_history(history, history_budget)
    used_tokens = (
        fixed_tokens
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
