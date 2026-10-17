import tempfile
from pathlib import Path

from pynotebook.sources import chunk, extract


def test_estimate_tokens():
    assert extract.estimate_tokens("hello world") > 0


def test_chunk_simple():
    text = "a\n\nb\n\nc"
    chunks = chunk.chunk_text(text, chunk_tokens=10, overlap_tokens=1)
    assert len(chunks) >= 1


def test_extract_text_basic():
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "test.txt"
        p.write_text("hello world")
        content, size, ext, _mime, err = extract.extract_text(p)
        assert content == "hello world"
        assert size > 0
        assert ext == "txt"
        assert err is None
