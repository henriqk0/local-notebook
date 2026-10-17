from __future__ import annotations

from collections.abc import AsyncIterator


async def parse_sse_lines(lines: AsyncIterator[str]) -> AsyncIterator[str]:
    """Parse SSE data lines, ignoring comments/keep-alives.

    Yields the raw `data:` payload strings (not including the `data:` prefix).
    Stops when it sees the `[DONE]` sentinel.
    """
    async for raw in lines:
        line = raw.strip()
        if not line:
            continue
        if line.startswith(":"):
            continue
        if not line.startswith("data:"):
            continue
        payload = line[len("data:") :].strip()
        if payload == "[DONE]":
            return
        yield payload
