from pathlib import Path

import pytest

from pynotebook import config


def test_environment_key_is_not_persisted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    config_path = tmp_path / "config.toml"
    monkeypatch.setenv("OPENROUTER_API_KEY", "env-secret")
    cfg = config.load_config(config_path)
    assert cfg.api_key == "env-secret"

    config.save_config(cfg, config_path)
    assert config_path.exists()
    assert "env-secret" not in config_path.read_text(encoding="utf-8")

    # An explicitly saved key is persisted, while the env key stays in memory.
    cfg.set_api_key("saved-secret")
    config.save_config(cfg, config_path)
    raw = config_path.read_text(encoding="utf-8")
    assert "saved-secret" in raw
    assert "env-secret" not in raw

    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    reloaded = config.load_config(config_path)
    assert reloaded.api_key == "saved-secret"


def test_saved_key_survives_env_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    config_path = tmp_path / "config.toml"
    cfg = config.Config()
    cfg.set_api_key("file-secret")
    config.save_config(cfg, config_path)

    monkeypatch.setenv("OPENROUTER_API_KEY", "env-secret")
    loaded = config.load_config(config_path)
    assert loaded.api_key == "env-secret"
    # Saving while an env key is active must not overwrite the stored key.
    config.save_config(loaded, config_path)
    raw = config_path.read_text(encoding="utf-8")
    assert "file-secret" in raw
    assert "env-secret" not in raw


def test_config_file_and_dir_permissions(tmp_path: Path):
    config_path = tmp_path / "config.toml"
    config.save_config(config.Config(), config_path)
    assert config_path.stat().st_mode & 0o777 == 0o600
    assert config_path.parent.stat().st_mode & 0o777 == 0o700


def test_from_dict_coerces_invalid_values():
    cfg = config.Config.from_dict(
        {
            "context_token_budget": "nope",
            "ranger_command": 5,
            "system_prompt": "",
            "default_model": 7,
            "api_key": 42,
        }
    )
    assert cfg.context_token_budget == 24000
    assert cfg.ranger_command == "ranger"
    assert cfg.system_prompt == config.DEFAULT_SYSTEM_PROMPT
    assert cfg.default_model is None
    assert cfg.api_key is None


def test_from_dict_clamps_budget():
    assert (
        config.Config.from_dict({"context_token_budget": -5}).context_token_budget
        == 1000
    )
    assert (
        config.Config.from_dict({"context_token_budget": 10**9}).context_token_budget
        == config.MAX_CONTEXT_TOKEN_BUDGET
    )
    assert (
        config.Config.from_dict({"context_token_budget": True}).context_token_budget
        == 24000
    )


def test_malformed_config_file_falls_back_to_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    config_path = tmp_path / "config.toml"
    config_path.write_bytes(b"\xff\xfe not toml")
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    cfg = config.load_config(config_path)
    assert cfg.context_token_budget == 24000
    assert cfg.api_key is None
