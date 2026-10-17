from pynotebook import models
from pynotebook.llm import context


def make_source(sid, filename, text, enabled=True, status="ok"):
    return models.Source(
        id=sid,
        notebook_id=1,
        path=f"/tmp/{filename}",
        filename=filename,
        content_text=text,
        token_estimate=context.estimate_tokens(text),
        enabled=enabled,
        status=status,
    )


def test_sanitize_fts_query():
    assert context.sanitize_fts_query("hello world") == '"hello" OR "world"'
    assert context.sanitize_fts_query("what's up?") == '"what" OR "s" OR "up"'
    assert context.sanitize_fts_query("!!!") == ""


def test_full_text_when_under_budget():
    sources = [make_source(1, "a.txt", "hello world"), make_source(2, "b.txt", "more text")]
    result = context.build_context(
        "question",
        sources,
        [],
        "sys",
        token_budget=100000,
    )
    assert result.mode == "full"
    assert "=== SOURCE: a.txt ===" in result.sources_block
    assert "=== SOURCE: b.txt ===" in result.sources_block


def test_retrieval_when_over_budget():
    big = "word " * 100000
    sources = [make_source(1, "big.txt", big)]
    hits = [context.ChunkHit(1, "big.txt", "relevant chunk")]
    result = context.build_context(
        "question",
        sources,
        [],
        "sys",
        token_budget=100,
        search_chunks=lambda q: hits,
    )
    assert result.mode == "retrieval"
    assert "relevant chunk" in result.sources_block


def test_disabled_and_error_sources_excluded():
    sources = [
        make_source(1, "off.txt", "x", enabled=False),
        make_source(2, "err.txt", "y", status="error"),
    ]
    result = context.build_context("q", sources, [], "sys", token_budget=1000)
    assert result.sources_block == ""


def test_history_trimming_keeps_newest():
    history = [
        models.Message(role="user", content="old " * 1000),
        models.Message(role="assistant", content="new"),
    ]
    trimmed = context.trim_history(history, token_budget=5)
    assert trimmed[-1].content == "new"
    assert len(trimmed) == 1


def test_markdown_render_delimiters():
    block = context.build_sources_block_full([make_source(1, "doc.md", "body")])
    assert block.startswith("=== SOURCE: doc.md ===")
    assert block.endswith("=== END SOURCE: doc.md ===")
