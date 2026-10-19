from __future__ import annotations

CHARS_PER_TOKEN = 4


def estimate_tokens(text: str) -> int:
    """Conservative character-based token estimate."""
    if not text:
        return 0
    return (len(text) + CHARS_PER_TOKEN - 1) // CHARS_PER_TOKEN
