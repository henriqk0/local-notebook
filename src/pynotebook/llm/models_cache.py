from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class ModelInfo:
    id: str
    name: str = ""
    context_length: int = 0
    prompt_price: float = 0.0
    completion_price: float = 0.0
    is_free: bool = False

    @classmethod
    def from_api(cls, data: dict) -> ModelInfo:
        pricing = data.get("pricing") or {}
        try:
            prompt_price = float(pricing.get("prompt", 0) or 0)
        except (TypeError, ValueError):
            prompt_price = 0.0
        try:
            completion_price = float(pricing.get("completion", 0) or 0)
        except (TypeError, ValueError):
            completion_price = 0.0
        return cls(
            id=data.get("id", ""),
            name=data.get("name", "") or data.get("id", ""),
            context_length=int(data.get("context_length") or 0),
            prompt_price=prompt_price,
            completion_price=completion_price,
            is_free=(prompt_price == 0.0 and completion_price == 0.0),
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
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            self.models = []
            return self.models
        fetched_at = data.get("fetched_at", 0)
        if time.time() - fetched_at > self.ttl_seconds:
            self.models = []
            return self.models
        self.models = [ModelInfo(**m) for m in data.get("models", [])]
        return self.models

    def save(self, models: list[ModelInfo]) -> None:
        self.models = models
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "fetched_at": time.time(),
            "models": [m.__dict__ for m in models],
        }
        self.path.write_text(json.dumps(payload), encoding="utf-8")

    def is_stale(self) -> bool:
        if not self.path.exists():
            return True
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return True
        return time.time() - data.get("fetched_at", 0) > self.ttl_seconds

    @staticmethod
    def filter_models(models: list[ModelInfo], query: str) -> list[ModelInfo]:
        if not query:
            return list(models)
        q = query.lower()
        return [m for m in models if q in m.id.lower() or q in m.name.lower()]
