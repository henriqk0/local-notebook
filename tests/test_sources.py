import tempfile
from pathlib import Path

import pytest

from pynotebook.sources import chunk, extract, process


def test_estimate_tokens():
    assert extract.estimate_tokens("hello world") > 0


def test_chunk_simple():
    text = "a\n\nb\n\nc"
    chunks = chunk.chunk_text(text, chunk_tokens=10, overlap_tokens=1)
    assert len(chunks) >= 1


def test_chunk_long_paragraph_is_split():
    text = "x" * 20000
    chunks = chunk.chunk_text(text, chunk_tokens=10, overlap_tokens=1)
    assert len(chunks) > 1
    assert all(len(c) <= 40 for c in chunks)


def test_chunk_huge_overlap_is_capped():
    chunks = chunk.chunk_text("a" * 100, chunk_tokens=2, overlap_tokens=1000)
    assert len(chunks) >= 1
    assert all(len(c) <= 8 for c in chunks)


def test_chunk_invalid_chunk_tokens():
    with pytest.raises(ValueError):
        chunk.chunk_text("abc", chunk_tokens=0)


def test_extract_text_basic():
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "test.txt"
        p.write_text("hello world")
        content, size, ext, _mime, err = extract.extract_text(p)
        assert content == "hello world"
        assert size > 0
        assert ext == "txt"
        assert err is None


def test_extract_oversized_file_is_rejected():
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "big.txt"
        with p.open("wb") as f:
            f.truncate(extract.MAX_FILE_SIZE_BYTES + 1)
        content, _size, _ext, _mime, err = extract.extract_text(p)
        assert content == ""
        assert err is not None and "larger" in err.lower()


def test_extract_corrupt_docx_returns_error():
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "bad.docx"
        p.write_bytes(b"definitely not a zip archive")
        content, _size, _ext, _mime, err = extract.extract_text(p)
        assert content == ""
        assert err is not None


def test_empty_file_becomes_error_source():
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "empty.txt"
        p.write_text("")
        source, chunks = process.build_source_from_path(1, p)
        assert source.status == "error"
        assert source.enabled is False
        assert source.error == process.NO_TEXT_ERROR
        assert chunks == []
