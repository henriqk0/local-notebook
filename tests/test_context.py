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
    assert context.sanitize_fts_query("hello world") == '"hello"* OR "world"*'
    assert context.sanitize_fts_query("what's up?") == '"what"* OR "s"* OR "up"*'
    assert context.sanitize_fts_query("!!!") == ""


def test_sanitize_fts_query_dedupes_and_caps():
    query = "hello hello " + " ".join(f"w{i}" for i in range(50))
    result = context.sanitize_fts_query(query)
    assert result.count(" OR ") == context.MAX_FTS_TOKENS - 1
    assert result.startswith('"hello"* OR "w0"*')


def test_full_text_when_under_budget():
    sources = [
        make_source(1, "a.txt", "hello world"),
        make_source(2, "b.txt", "more text"),
    ]
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


def test_history_trimming_keeps_newest_turn():
    history = [
        models.Message(role="user", content="old " * 1000),
        models.Message(role="assistant", content="new"),
    ]
    trimmed = context.trim_history(history, token_budget=5)
    assert trimmed[-1].content == "new"
    assert len(trimmed) == 2
    assert trimmed[0].role == "user"
    assert context.estimate_tokens(trimmed[0].content) <= 4


def test_history_trimming_keeps_complete_turns():
    history = [
        models.Message(role="user", content="old " * 1000),
        models.Message(role="assistant", content="old answer"),
        models.Message(role="user", content="new question"),
        models.Message(role="assistant", content="new answer"),
    ]
    trimmed = context.trim_history(history, token_budget=100)
    assert [m.content for m in trimmed] == ["new question", "new answer"]


def test_history_trimming_never_emits_empty_messages():
    history = [
        models.Message(role="user", content="long question " * 100),
        models.Message(role="assistant", content="short answer"),
    ]
    trimmed = context.trim_history(history, token_budget=2)
    assert all(m.content for m in trimmed)
    assert trimmed[-1].role == "assistant"

    assert context.trim_history(history, token_budget=0) == []


def test_markdown_render_delimiters():
    block = context.build_sources_block_full([make_source(1, "doc.md", "body")])
    assert block.startswith("=== SOURCE: doc.md ===")
    assert block.endswith("=== END SOURCE: doc.md ===")
