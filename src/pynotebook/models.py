from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime


@dataclass
class Notebook:
    id: int | None = None
    name: str = ""
    description: str = ""
    model: str | None = None
    created_at: datetime | None = None
    updated_at: datetime | None = None


@dataclass
class Source:
    id: int | None = None
    notebook_id: int = 0
    path: str = ""
    filename: str = ""
    mime: str | None = None
    ext: str | None = None
    size: int = 0
    content_text: str = ""
    token_estimate: int = 0
    enabled: bool = True
    status: str = "ok"  # ok, error, pending
    error: str | None = None
    added_at: datetime | None = None


@dataclass
class Chunk:
    id: int | None = None
    source_id: int = 0
    idx: int = 0
    text: str = ""


@dataclass
class Message:
    id: int | None = None
    notebook_id: int = 0
    role: str = ""  # user, assistant, error
    content: str = ""
    model: str | None = None
    created_at: datetime | None = None
