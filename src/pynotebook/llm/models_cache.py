from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


def _as_float(value: Any, default: float = 0.0) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _as_int(value: Any, default: int = 0, minimum: int = 0) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return max(parsed, minimum)


@dataclass
class ModelInfo:
    id: str
    name: str = ""
    context_length: int = 0
    prompt_price: float = 0.0
    completion_price: float = 0.0
    is_free: bool = False

    @classmethod
    def from_api(cls, data: dict[str, Any]) -> ModelInfo:
        pricing = data.get("pricing")
        if not isinstance(pricing, dict):
            pricing = {}

        prompt_price = _as_float(pricing.get("prompt"))
        completion_price = _as_float(pricing.get("completion"))
        model_id = data.get("id") if isinstance(data.get("id"), str) else ""
        name = data.get("name") if isinstance(data.get("name"), str) else model_id
        return cls(
            id=model_id,
            name=name,
            context_length=_as_int(data.get("context_length")),
            prompt_price=prompt_price,
            completion_price=completion_price,
            is_free=(prompt_price == 0.0 and completion_price == 0.0),
        )

    @classmethod
    def from_cache(cls, data: dict[str, Any]) -> ModelInfo:
        """Rebuild from the local cache, coercing any corrupted values."""
        model_id = data.get("id") if isinstance(data.get("id"), str) else ""
        name = data.get("name") if isinstance(data.get("name"), str) else model_id
        prompt_price = _as_float(data.get("prompt_price"))
        completion_price = _as_float(data.get("completion_price"))
        is_free = data.get("is_free")
        return cls(
            id=model_id,
            name=name,
            context_length=_as_int(data.get("context_length")),
            prompt_price=prompt_price,
            completion_price=completion_price,
            is_free=bool(is_free)
            if isinstance(is_free, bool)
            else (prompt_price == 0.0 and completion_price == 0.0),
        )


@dataclass
class ModelCache:
    path: Path
    ttl_seconds: int = 24 * 3600
    models: list[ModelInfo] = field(default_factory=list)

    def load(self) -> list[ModelInfo]:
        if not self.path.exists():
            self.models = []
            return self.models

        try:
            payload = json.loads(self.path.read_text(encoding="utf-8"))
            if not isinstance(payload, dict):
                raise TypeError("cache root must be an object")
            fetched_at = _as_float(payload.get("fetched_at"))
            if time.time() - fetched_at > self.ttl_seconds:
                self.models = []
                return self.models

            raw_models = payload.get("models", [])
            if not isinstance(raw_models, list):
                raise TypeError("cache models must be a list")
            self.models = [
                ModelInfo.from_cache(raw) for raw in raw_models if isinstance(raw, dict)
            ]
        except (OSError, ValueError, TypeError, UnicodeError):
            self.models = []

        return self.models

    def save(self, models: list[ModelInfo]) -> None:
        self.models = models
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "fetched_at": time.time(),
            "models": [m.__dict__ for m in models],
        }
        tmp_path = self.path.with_suffix(self.path.suffix + ".tmp")
        try:
            tmp_path.write_text(json.dumps(payload), encoding="utf-8")
            os.replace(tmp_path, self.path)
        except OSError:
            try:
                tmp_path.unlink(missing_ok=True)
            except OSError:
                pass
            raise

    def is_stale(self) -> bool:
        if not self.path.exists():
            return True
        try:
            payload = json.loads(self.path.read_text(encoding="utf-8"))
            if not isinstance(payload, dict):
                return True
            fetched_at = _as_float(payload.get("fetched_at"))
        except (OSError, ValueError, TypeError, UnicodeError):
            return True
        return time.time() - fetched_at > self.ttl_seconds

    @staticmethod
    def filter_models(models: list[ModelInfo], query: str) -> list[ModelInfo]:
        if not query:
            return list(models)
        q = query.lower()
        return [m for m in models if q in m.id.lower() or q in m.name.lower()]
