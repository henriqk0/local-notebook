import json
import time
from pathlib import Path

from pynotebook.llm.models_cache import ModelCache, ModelInfo


def test_model_cache_roundtrip(tmp_path: Path):
    cache = ModelCache(tmp_path / "models.json")
    models = [
        ModelInfo(
            id="a/b",
            name="AB",
            context_length=4096,
            prompt_price=0.0,
            completion_price=0.0,
            is_free=True,
        )
    ]
    cache.save(models)
    loaded = ModelCache(tmp_path / "models.json").load()
    assert len(loaded) == 1
    assert loaded[0].id == "a/b"
    assert loaded[0].is_free


def test_filter_models():
    models = [
        ModelInfo(id="openai/gpt-4", name="GPT-4"),
        ModelInfo(id="anthropic/claude", name="Claude"),
    ]
    assert len(ModelCache.filter_models(models, "gpt")) == 1
    assert len(ModelCache.filter_models(models, "")) == 2


def test_from_api_pricing():
    m = ModelInfo.from_api(
        {
            "id": "x/y",
            "name": "XY",
            "context_length": 1000,
            "pricing": {"prompt": "0.5", "completion": "1.5"},
        }
    )
    assert m.prompt_price == 0.5
    assert not m.is_free


def test_malformed_cache_returns_empty(tmp_path: Path):
    path = tmp_path / "models.json"
    path.write_text("{not valid json", encoding="utf-8")
    assert ModelCache(path).load() == []

    path.write_text('{"fetched_at": "not a number", "models": []}', encoding="utf-8")
    assert ModelCache(path).load() == []

    path.write_text(
        json.dumps({"fetched_at": time.time(), "models": "nope"}), encoding="utf-8"
    )
    assert ModelCache(path).load() == []


def test_cache_ignores_unknown_fields(tmp_path: Path):
    path = tmp_path / "models.json"
    path.write_text(
        json.dumps(
            {
                "fetched_at": time.time(),
                "models": [{"id": "a/b", "name": "AB", "surprise": True}],
            }
        ),
        encoding="utf-8",
    )
    loaded = ModelCache(path).load()
    assert len(loaded) == 1
    assert loaded[0].id == "a/b"


def test_from_api_malformed_fields():
    m = ModelInfo.from_api(
        {
            "id": "a/b",
            "pricing": {"prompt": "abc", "completion": None},
            "context_length": "nope",
        }
    )
    assert m.prompt_price == 0.0
    assert m.completion_price == 0.0
    assert m.context_length == 0
    assert m.is_free


def test_cache_coerces_corrupted_values(tmp_path: Path):
    path = tmp_path / "models.json"
    path.write_text(
        json.dumps(
            {
                "fetched_at": time.time(),
                "models": [
                    {
                        "id": "a/b",
                        "name": "AB",
                        "context_length": "nope",
                        "prompt_price": "free",
                        "completion_price": None,
                        "is_free": "yes",
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    loaded = ModelCache(path).load()
    assert len(loaded) == 1
    assert loaded[0].context_length == 0
    assert loaded[0].prompt_price == 0.0
    assert loaded[0].completion_price == 0.0
    assert loaded[0].is_free is True
