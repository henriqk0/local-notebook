from pathlib import Path

from pynotebook.sources import process, ranger


def test_read_chooser_file(tmp_path: Path):
    f1 = tmp_path / "a.txt"
    f1.write_text("hello")
    f2 = tmp_path / "b.txt"
    f2.write_text("world")
    chooser = tmp_path / "chooser.txt"
    chooser.write_text(f"{f1}\n\n{f2}\n{f1}\n")
    paths = ranger.read_chooser_file(chooser)
    assert f1.resolve() in paths
    assert f2.resolve() in paths
    # dedup
    paths = ranger.dedupe_paths(paths)
    assert len(paths) == 2


def test_pick_files_with_mock(monkeypatch, tmp_path: Path):
    f1 = tmp_path / "a.txt"
    f1.write_text("hello")

    def fake_run(cmd, check=False):
        # cmd == [ranger, --choosefiles=<tmpfile>]
        arg = cmd[1]
        tmpfile = Path(arg.split("=", 1)[1])
        tmpfile.write_text(str(f1) + "\n")
        class R:
            returncode = 0
        return R()

    monkeypatch.setattr(ranger.subprocess, "run", fake_run)
    paths = ranger.pick_files("ranger")
    assert len(paths) == 1
    assert paths[0] == f1.resolve()


def test_build_source_from_path(tmp_path: Path):
    f1 = tmp_path / "doc.md"
    f1.write_text("hello world")
    source, chunks = process.build_source_from_path(1, f1)
    assert source.filename == "doc.md"
    assert source.status == "ok"
    assert source.enabled is True
    assert len(chunks) >= 1


def test_build_source_unsupported(tmp_path: Path):
    f1 = tmp_path / "blob.bin"
    f1.write_bytes(b"\x00\x01\x02")
    source, _chunks = process.build_source_from_path(1, f1)
    assert source.status == "error"
    assert source.enabled is False
