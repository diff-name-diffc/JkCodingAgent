"""模型原始响应到动作执行边界：真实 browser-use/Chrome，不访问外部模型。"""

import json

import httpx
import pytest

from test_model_protocol import completion_response, fixture_model, system_text
from test_sidecar import params, runtime as runtime_fixture

runtime = runtime_fixture


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "content",
    [
        '<think>fixture reasoning</think>{"action":[{"navigate":{"url":"https://example.com"}}]}',
        json.dumps({"action": [{"navigate": {"url": {"invalid": "object"}}}]}),
    ],
)
async def test_invalid_model_output_never_reaches_browser_actions(runtime, content):
    requests = []

    def respond(request):
        requests.append(json.loads(request.content))
        return completion_response(content)

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        result = await runtime.explore(params(), llm=model)
    assert not result["success"]
    assert result["iterations"] == 3 and len(requests) == 3
    assert all(body["response_format"] == {"type": "json_object"} for body in requests)
    assert not any(event.get("kind") == "action_started" for event in runtime.events)
    assert "ValidationError" in result["result"]
    assert "fixture reasoning" not in result["result"]
    assert "HTTP 502" not in result["result"]


@pytest.mark.asyncio
async def test_valid_action_schema_reaches_execution_and_completes(runtime):
    requests = []

    def respond(request):
        requests.append(json.loads(request.content))
        return completion_response(
            json.dumps(
                {
                    "evaluation_previous_goal": "Fixture complete",
                    "memory": "fixture",
                    "next_goal": "Finish",
                    "action": [{"done": {"text": "fixture complete", "success": True}}],
                }
            )
        )

    model = fixture_model()
    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model.http_client = client
        result = await runtime.explore(params(), llm=model)
    assert result["success"] and result["result"] == "fixture complete"
    assert result["iterations"] == 1 and len(requests) == 1
    assert requests[0]["response_format"] == {"type": "json_object"}
    assert "done action's text field" in system_text(requests[0]["messages"][0])
    started = [
        event for event in runtime.events if event.get("kind") == "action_started"
    ]
    finished = [
        event for event in runtime.events if event.get("kind") == "action_finished"
    ]
    assert len(started) == len(finished) == 1
    assert started[0]["data"]["toolName"] == "done"
    assert finished[0]["data"]["success"]
