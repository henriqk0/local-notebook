import json

import httpx
import pytest

from pynotebook.llm import openrouter, sse


@pytest.mark.asyncio
async def test_parse_sse_lines():
    async def gen():
        for line in [
            ": keep-alive",
            "",
            'data: {"a": 1}',
            "data: [DONE]",
            'data: {"never": true}',
        ]:
            yield line

    out = [x async for x in sse.parse_sse_lines(gen())]
    assert out == ['{"a": 1}']


@pytest.mark.asyncio
async def test_stream_chat_success():
    sse_body = (
        'data: {"choices":[{"delta":{"content":"Hel"}}]}\n\n'
        'data: {"choices":[{"delta":{"content":"lo"}}]}\n\n'
        "data: [DONE]\n\n"
    )

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["Authorization"] == "Bearer testkey"
        return httpx.Response(
            200, text=sse_body, headers={"content-type": "text/event-stream"}
        )

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("testkey", client=client)
        chunks = []
        async for tok in c.stream_chat(
            "test/model", [openrouter.ChatMessage("user", "hi")]
        ):
            chunks.append(tok)
    assert "".join(chunks) == "Hello"


@pytest.mark.asyncio
async def test_stream_chat_401():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(401, text="unauthorized")

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("bad", client=client)
        with pytest.raises(openrouter.OpenRouterError) as exc:
            async for _ in c.stream_chat("m", [openrouter.ChatMessage("user", "hi")]):
                pass
        assert exc.value.status_code == 401


@pytest.mark.asyncio
async def test_list_models():
    body = json.dumps(
        {
            "data": [
                {
                    "id": "abc/model",
                    "name": "Model",
                    "context_length": 8192,
                    "pricing": {"prompt": "0.1", "completion": "0.2"},
                },
            ]
        }
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, text=body)

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("k", client=client)
        models = await c.list_models()
    assert models[0]["id"] == "abc/model"


@pytest.mark.asyncio
async def test_list_models_malformed_response():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"data": "not a list"})

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("k", client=client)
        with pytest.raises(openrouter.OpenRouterError):
            await c.list_models()


@pytest.mark.asyncio
async def test_stream_chat_error_event_raises():
    sse_body = 'data: {"error": {"message": "bad upstream thing"}}\n\n'

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200, text=sse_body, headers={"content-type": "text/event-stream"}
        )

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("k", client=client)
        with pytest.raises(openrouter.OpenRouterError) as exc:
            async for _ in c.stream_chat("m", [openrouter.ChatMessage("user", "hi")]):
                pass
    assert "bad upstream thing" in exc.value.message


@pytest.mark.asyncio
async def test_error_body_is_bounded():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(500, text="x" * 200_000)

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as client:
        c = openrouter.OpenRouterClient("k", client=client)
        with pytest.raises(openrouter.OpenRouterError) as exc:
            await c.list_models()
    assert len(exc.value.message) < 400
