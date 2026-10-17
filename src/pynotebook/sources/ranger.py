from __future__ import annotations

import os
import subprocess
import tempfile
from pathlib import Path


def run_ranger(ranger_command: str, tmpfile: Path) -> subprocess.CompletedProcess:
    cmd = [ranger_command, f"--choosefiles={tmpfile}"]
    return subprocess.run(cmd, check=False)


def read_chooser_file(tmpfile: Path) -> list[Path]:
    paths: list[Path] = []
    try:
        with tmpfile.open("r", encoding="utf-8", errors="replace") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                p = Path(line)
                if p.is_dir():
                    continue
                try:
                    paths.append(p.resolve())
                except OSError:
                    continue
    except (OSError, ValueError):
        pass
    return paths


def dedupe_paths(paths: list[Path]) -> list[Path]:
    seen: set[str] = set()
    out: list[Path] = []
    for p in paths:
        s = str(p)
        if s in seen:
            continue
        seen.add(s)
        out.append(p)
    return out


def pick_files(ranger_command: str) -> list[Path]:
    fd, name = tempfile.mkstemp(prefix="pynotebook_ranger_", suffix=".txt")
    os.close(fd)
    tmpfile = Path(name)
    try:
        try:
            run_ranger(ranger_command, tmpfile)
        except (OSError, subprocess.SubprocessError):
            return []
        paths = read_chooser_file(tmpfile)
        paths = dedupe_paths(paths)
        return paths
    finally:
        try:
            tmpfile.unlink(missing_ok=True)
        except OSError:
            pass
