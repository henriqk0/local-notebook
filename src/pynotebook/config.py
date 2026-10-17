from __future__ import annotations

import os
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

try:
    import tomllib  # Python 3.11+
except ImportError:  # pragma: no cover
    import tomli as tomllib  # type: ignore

import tomli_w


def get_config_dir() -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    if config_home:
        base = Path(config_home)
    else:
        base = Path.home() / ".config"
    return base / "pynotebook"


def get_data_dir() -> Path:
    data_home = os.environ.get("XDG_DATA_HOME")
    if data_home:
        base = Path(data_home)
    else:
        base = Path.home() / ".local" / "share"
    return base / "pynotebook"


DEFAULT_SYSTEM_PROMPT = (
    "You are a helpful assistant that answers using only the provided sources. "
    "Cite sources as [filename]. If the sources do not contain the answer, "
    "say so clearly."
)


@dataclass
class Config:
    api_key: str | None = None
    default_model: str | None = None
    ranger_command: str = "ranger"
    context_token_budget: int = 24000
    system_prompt: str = DEFAULT_SYSTEM_PROMPT
    _config_path: Path | None = field(default=None, repr=False)

    def to_dict(self) -> dict[str, Any]:
        data = asdict(self)
        data.pop("_config_path", None)
        return data

    @classmethod
    def from_dict(cls, data: dict[str, Any], config_path: Path | None = None) -> Config:
        return cls(
            api_key=data.get("api_key"),
            default_model=data.get("default_model"),
            ranger_command=data.get("ranger_command", "ranger"),
            context_token_budget=data.get("context_token_budget", 24000),
            system_prompt=data.get("system_prompt", DEFAULT_SYSTEM_PROMPT),
            _config_path=config_path,
        )


def load_config(config_path: Path | None = None) -> Config:
    if config_path is None:
        config_path = get_config_dir() / "config.toml"

    config_data: dict[str, Any] = {}

    if config_path.exists():
        try:
            with config_path.open("rb") as f:
                config_data = tomllib.load(f)
        except (OSError, ValueError):
            config_data = {}

    config = Config.from_dict(config_data, config_path=config_path)

    # API key priority: env var over config file
    env_key = os.environ.get("OPENROUTER_API_KEY")
    if env_key:
        config.api_key = env_key

    return config


def save_config(config: Config, config_path: Path | None = None) -> None:
    if config_path is None:
        config_path = config._config_path or (get_config_dir() / "config.toml")

    config_path.parent.mkdir(parents=True, exist_ok=True)
    data = {
        k: v
        for k, v in config.to_dict().items()
        if v is not None or k in ("ranger_command", "context_token_budget", "system_prompt")
    }
    with config_path.open("wb") as f:
        tomli_w.dump(data, f)

    # Set file mode to 0600 for security
    try:
        os.chmod(config_path, 0o600)
    except OSError:
        pass
