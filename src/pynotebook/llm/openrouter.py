from __future__ import annotations

import json
from collections.abc import AsyncIterator
from dataclasses import dataclass

import httpx

from .sse import parse_sse_lines

BASE_URL = "https://openrouter.ai/api/v1"
APP_TITLE = "pynotebook"
APP_REFERER = "https://github.com/pynotebook"


class OpenRouterError(Exception):
    def __init__(self, message: str, status_code: int | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.status_code = status_code


@dataclass
class ChatMessage:
    role: str
    content: str

    def to_dict(self) -> dict[str, str]:
        return {"role": self.role, "content": self.content}


def _headers(api_key: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "HTTP-Referer": APP_REFERER,
        "X-Title": APP_TITLE,
    }


def _error_message(status_code: int, body: str) -> str:
    if status_code == 401:
        return "Invalid API key (401). Check your OPENROUTER_API_KEY."
    if status_code == 402:
        return "Insufficient credits (402). Add credits to your OpenRouter account."
    if status_code == 429:
        return "Rate limited (429). Please wait and try again."
    return f"OpenRouter error {status_code}: {body[:300]}"


class OpenRouterClient:
    def __init__(self, api_key: str, client: httpx.AsyncClient | None = None) -> None:
        self.api_key = api_key
        self._client = client
        self._owns_client = client is None

    def _get_client(self) -> httpx.AsyncClient:
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=httpx.Timeout(60.0, connect=10.0))
        return self._client

    async def aclose(self) -> None:
        if self._client is not None and self._owns_client:
            await self._client.aclose()
            self._client = None

    async def list_models(self) -> list[dict]:
        client = self._get_client()
        try:
            resp = await client.get(f"{BASE_URL}/models", headers=_headers(self.api_key))
        except httpx.HTTPError as e:
            raise OpenRouterError(f"Network error: {e}") from e
        if resp.status_code != 200:
            raise OpenRouterError(
                _error_message(resp.status_code, resp.text), resp.status_code
            )
        data = resp.json()
        return data.get("data", [])

    async def stream_chat(
        self, model: str, messages: list[ChatMessage]
    ) -> AsyncIterator[str]:
        client = self._get_client()
        payload = {
            "model": model,
            "messages": [m.to_dict() for m in messages],
            "stream": True,
        }
        try:
            async with client.stream(
                "POST",
                f"{BASE_URL}/chat/completions",
                headers=_headers(self.api_key),
                json=payload,
            ) as resp:
                if resp.status_code != 200:
                    body = await resp.aread()
                    text = body.decode("utf-8", errors="replace")
                    raise OpenRouterError(
                        _error_message(resp.status_code, text), resp.status_code
                    )
                async for payload_line in parse_sse_lines(resp.aiter_lines()):
                    try:
                        obj = json.loads(payload_line)
                    except json.JSONDecodeError:
                        continue
                    choices = obj.get("choices") or []
                    if not choices:
                        continue
                    delta = choices[0].get("delta") or {}
                    content = delta.get("content")
                    if content:
                        yield content
        except httpx.HTTPError as e:
            raise OpenRouterError(f"Network error: {e}") from e
