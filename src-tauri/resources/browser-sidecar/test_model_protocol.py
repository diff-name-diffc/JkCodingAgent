"""模型协议回归：真实 SDK 与内存 HTTP 传输，不请求外部服务。"""

import asyncio
import json

import bootstrap  # noqa: F401
import httpx
import pytest
from browser_use.llm.exceptions import ModelProviderError, ModelRateLimitError
from browser_use.llm.messages import (
    ContentPartImageParam,
    ContentPartTextParam,
    ImageURL,
    SystemMessage,
    UserMessage,
)
from pydantic import BaseModel, Field

from explorer import model_from_config


class Answer(BaseModel):
    answer: str = Field(description='返回中文结果，保留 "引号" 和换行\n。')


def fixture_model():
    return model_from_config(
        {
            "model": "fixture-model",
            "apiBase": "http://fixture.invalid/v1",
            "apiKey": "fixture-key",
        }
    )


def completion_response(content):
    return httpx.Response(
        200,
        json={
            "id": "fixture",
            "object": "chat.completion",
            "created": 0,
            "model": "fixture-model",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": content},
                    "finish_reason": "stop",
                }
            ],
        },
    )


def system_text(message):
    content = message["content"]
    return (
        content if isinstance(content, str) else "\n".join(p["text"] for p in content)
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("system_style", ["string", "parts", "missing"])
async def test_schema_is_valid_json_and_does_not_mutate_reused_messages(system_style):
    requests = []
    instruction = "Preserve this system instruction."
    messages = []
    if system_style != "missing":
        content = (
            instruction
            if system_style == "string"
            else [ContentPartTextParam(text=instruction)]
        )
        messages.append(SystemMessage(content=content))
    messages.append(
        UserMessage(
            content=[
                ContentPartTextParam(text="Inspect this fixture."),
                ContentPartImageParam(
                    image_url=ImageURL(url="data:image/png;base64,Zml4dHVyZQ==")
                ),
            ]
        )
    )
    original = [message.model_dump() for message in messages]

    def respond(request):
        requests.append(json.loads(request.content))
        return completion_response('{"answer":"ready"}')

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        for _ in range(2):
            result = await model.ainvoke(messages, output_format=Answer)
            assert result.completion.answer == "ready"
    assert len(requests) == 2
    assert [message.model_dump() for message in messages] == original
    for body in requests:
        assert body["response_format"] == {"type": "json_object"}
        assert body["messages"][0]["role"] == "system"
        text = system_text(body["messages"][0])
        assert text.count("<json_schema>") == 1
        assert "done action" not in text
        assert "browser actions" not in text
        schema = json.loads(text.split("<json_schema>")[1].split("</json_schema>")[0])
        assert schema["type"] == "object"
        assert schema["properties"]["answer"]["type"] == "string"
        assert "返回中文结果" in schema["properties"]["answer"]["description"]
        if system_style != "missing":
            assert instruction in text
        user = next(m for m in body["messages"] if m["role"] == "user")
        assert user["content"][0]["text"] == "Inspect this fixture."
        assert user["content"][1]["image_url"]["url"].endswith("Zml4dHVyZQ==")


@pytest.mark.asyncio
async def test_unstructured_call_preserves_plain_text_protocol():
    requests = []

    def respond(request):
        requests.append(json.loads(request.content))
        return completion_response("Plain answer, no JSON required.")

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        result = await model.ainvoke([UserMessage(content="Summarize this page.")])
    assert result.completion == "Plain answer, no JSON required."
    assert len(requests) == 1
    assert "response_format" not in requests[0]
    assert requests[0]["messages"] == [
        {"role": "user", "content": "Summarize this page."}
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("status", [429, 502])
async def test_http_failure_preserves_status_and_existing_retry_budget(status):
    calls = 0

    def respond(request):
        nonlocal calls
        calls += 1
        return httpx.Response(
            status,
            headers={"retry-after-ms": "1"},
            json={"error": {"message": "fixture provider unavailable"}},
        )

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        with pytest.raises(ModelProviderError) as failed:
            await model.ainvoke([UserMessage(content="fixture")], output_format=Answer)
    assert failed.value.status_code == status
    assert "fixture provider unavailable" in str(failed.value)
    assert isinstance(failed.value, ModelRateLimitError) == (status == 429)
    assert calls == 2  # 既有 max_retries=1；适配层不能另加重试。


@pytest.mark.asyncio
async def test_cancellation_interrupts_request_without_retry():
    calls = 0
    started = asyncio.Event()
    cancelled = asyncio.Event()

    async def respond(request):
        nonlocal calls
        calls += 1
        started.set()
        try:
            await asyncio.Event().wait()
        finally:
            cancelled.set()

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        task = asyncio.create_task(
            model.ainvoke([UserMessage(content="fixture")], output_format=Answer)
        )
        try:
            await asyncio.wait_for(started.wait(), timeout=2)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
    assert cancelled.is_set() and calls == 1
