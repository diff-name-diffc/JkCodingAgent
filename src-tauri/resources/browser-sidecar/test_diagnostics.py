"""故障证据回归：真实 SDK、脚本模型，不访问真实模型服务。"""

import asyncio
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
import httpx
from openai import BadRequestError
from pydantic import BaseModel, ValidationError

from bootstrap import register_secret
from browser_use.llm.exceptions import ModelOutputTruncatedError, ModelProviderError
from browser_use.llm.messages import UserMessage
from browser_use.llm.openai.chat import ChatOpenAI
from diagnostics import RunDiagnostics, exception_diagnostic, safe_diagnostic
from explorer import PersistentAgent
from test_sidecar import ScriptedModel, params, runtime as runtime_fixture

runtime = runtime_fixture


def test_empty_exception_preserves_type_and_chained_cause_without_secret():
    register_secret("diagnostic-fixture-secret")
    try:
        try:
            raise ValueError("gateway rejected diagnostic-fixture-secret")
        except ValueError as cause:
            raise AssertionError() from cause
    except AssertionError as error:
        message = exception_diagnostic(error)
    assert message.startswith("AssertionError")
    assert "ValueError: gateway rejected [REDACTED]" in message
    assert "diagnostic-fixture-secret" not in message


def test_validation_diagnostics_omit_input_and_internal_model_content():
    class Action(BaseModel):
        index: int

    with pytest.raises(ValidationError) as failed:
        Action.model_validate({"index": {"thinking": "private-thought-sentinel"}})
    message = exception_diagnostic(failed.value)
    assert "ValidationError" in message and "index" in message
    assert "模型结构化输出" not in message
    assert "private-thought-sentinel" not in message
    assert "input_value" not in message
    for raw in [
        "<think>private-thought-sentinel</think> rejected response",
        '{"thinking":"private-thought-sentinel", "error":"invalid action"}',
        "{'memory': 'private-thought-sentinel', 'error': 'invalid action'}",
        "input_value={'thinking': 'private-thought-sentinel'}, input_type=dict",
    ]:
        assert "private-thought-sentinel" not in safe_diagnostic(raw)
    assert len(safe_diagnostic("x" * 50000)) <= 2001


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("content", "expected", "location"),
    [
        ("<think>private-response-sentinel</think>", "JSON 解析失败", "<root>"),
        ('{"private-response-sentinel": "ignored"}', "校验失败", "action"),
    ],
)
async def test_real_sdk_local_validation_is_not_reported_as_http_502(
    content, expected, location
):
    class AgentOutput(BaseModel):
        action: str

    def respond(request):
        return httpx.Response(
            200,
            json={
                "id": "fixture-response",
                "created": 0,
                "model": "fixture-model",
                "object": "chat.completion",
                "choices": [
                    {
                        "index": 0,
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": content},
                    }
                ],
            },
        )

    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model = ChatOpenAI(model="fixture-model", api_key="fixture", http_client=client)
        with pytest.raises(ModelProviderError) as failed:
            await model.ainvoke([UserMessage(content="fixture")], AgentOutput)
    assert failed.value.status_code == 502  # SDK 合成状态，不是 HTTP 响应。
    message = exception_diagnostic(failed.value)
    assert expected in message and "AgentOutput" in message
    assert location in message and "ValidationError" in message
    assert "HTTP" not in message and "ModelProviderError" not in message
    assert "private-response-sentinel" not in message and "input_value" not in message
    assert "errors.pydantic.dev" not in message
    assert message.count("Invalid JSON") <= 1


@pytest.mark.asyncio
async def test_real_sdk_http_502_keeps_response_status_and_server_message():
    def respond(request):
        return httpx.Response(
            502, json={"error": {"message": "fixture gateway unavailable"}}
        )

    async with httpx.AsyncClient(transport=httpx.MockTransport(respond)) as client:
        model = ChatOpenAI(
            model="fixture-model", api_key="fixture", http_client=client, max_retries=0
        )
        with pytest.raises(ModelProviderError) as failed:
            await model.ainvoke([UserMessage(content="fixture")])
    message = exception_diagnostic(failed.value)
    assert "ModelProviderError (HTTP 502)" in message
    assert message.count("fixture gateway unavailable") == 1
    assert "结构化输出" not in message


@pytest.mark.parametrize("status", [400, 429, 500, 502])
def test_sdk_status_without_http_response_is_not_an_http_error(status):
    cause = RuntimeError("fixture local adapter failure")
    error = ModelProviderError(str(cause), status_code=status)
    error.__cause__ = cause
    message = exception_diagnostic(error)
    assert "HTTP" not in message
    assert "fixture local adapter failure" in message


def test_sdk_output_truncation_is_not_an_http_400():
    error = ModelOutputTruncatedError(
        "Model output was truncated at max_completion_tokens=4096"
    )
    assert error.status_code == 400
    message = exception_diagnostic(error)
    assert "ModelOutputTruncatedError" in message
    assert "max_completion_tokens=4096" in message
    assert "HTTP" not in message


def fake_agent(*, done=False, success=False, final=None, errors=(), stopped=False):
    history = SimpleNamespace(
        is_done=lambda: done,
        is_successful=lambda: success,
        final_result=lambda: final,
        errors=lambda: errors,
    )
    return SimpleNamespace(
        history=history,
        state=SimpleNamespace(stopped=stopped, consecutive_failures=0, last_result=[]),
        settings=SimpleNamespace(max_failures=3),
    )


def test_empty_history_never_claims_step_budget_exhaustion():
    diagnostics = RunDiagnostics(lambda *args, **kwargs: None)
    success, text = diagnostics.result(fake_agent(), None, 200)
    assert not success and "未产生执行步骤" in text
    assert "步数限制" not in text


def test_final_outcome_distinguishes_stop_budget_and_model_declared_failure():
    diagnostics = RunDiagnostics(lambda *args, **kwargs: None)
    diagnostics.attempts = 2
    diagnostics.stop_reason = "Failed to complete task in maximum steps"
    success, text = diagnostics.result(fake_agent(), None, 2)
    assert not success and "步数限制（2 步）" in text
    success, text = diagnostics.result(fake_agent(stopped=True), None, 200)
    assert not success and "已停止" in text
    diagnostics.stop_reason = None
    success, text = diagnostics.result(
        fake_agent(done=True, final="site unavailable"), None, 200
    )
    assert not success and "site unavailable" in text
    success, text = diagnostics.result(
        fake_agent(
            done=True, success=True, final="complete", errors=["earlier failure"]
        ),
        None,
        200,
    )
    assert success and text == "complete"


def test_http_wrapper_reports_latest_message_once_and_omits_step_sentinel():
    diagnostics = RunDiagnostics(lambda *args, **kwargs: None)
    values = []
    for attempt in range(1, 4):
        message = (
            f"Invalid max_tokens value, valid range [1, 393216] (request_id: {attempt})"
        )
        body = {"error": {"message": message, "code": "400"}}
        raw = f"Error code: 400 - {body}"
        cause = BadRequestError(
            raw,
            response=httpx.Response(
                400, request=httpx.Request("POST", "http://fixture.invalid")
            ),
            body=body,
        )
        wrapped = ModelProviderError(raw, status_code=400)
        wrapped.__cause__ = cause
        diagnostics.attempts = attempt
        diagnostics.step_failed(wrapped)
        values.append(raw)
    diagnostics.stop_reason = "Failed to complete task in maximum steps"
    values.append(diagnostics.stop_reason)
    agent = fake_agent(errors=values)
    agent.state.consecutive_failures = 3
    success, text = diagnostics.result(agent, None, 3)
    assert not success and "连续 3 次失败" in text
    assert "ModelProviderError (HTTP 400)" in text
    assert text.count("Invalid max_tokens") == 1
    assert "request_id: 3" in text
    assert "request_id: 1" not in text and "request_id: 2" not in text
    assert "BadRequestError" not in text
    assert "maximum steps" not in text and "步数限制" not in text


def test_later_action_error_takes_precedence_over_an_earlier_recovered_exception():
    diagnostics = RunDiagnostics(lambda *args, **kwargs: None)
    diagnostics.attempts = 1
    diagnostics.step_failed(ValueError("earlier recovered failure"))
    diagnostics.attempts = 2
    agent = fake_agent(
        errors=["earlier recovered failure", "latest click target hidden"]
    )
    success, text = diagnostics.result(agent, None, 200)
    assert not success and "latest click target hidden" in text
    assert "earlier recovered failure" not in text


@pytest.mark.asyncio
async def test_real_sdk_model_failures_keep_specific_error_and_step_progress(runtime):
    class BrokenModel(ScriptedModel):
        model = "o-deepseek-v4-flash"

        async def ainvoke(self, *args, **kwargs):
            self.called.set()
            message = "Invalid max_tokens value, valid range [1, 393216]; diagnostic-model-secret"
            cause = BadRequestError(
                message,
                response=httpx.Response(
                    400, request=httpx.Request("POST", "http://fixture.invalid")
                ),
                body={"error": {"message": message}},
            )
            raise ModelProviderError(
                message,
                status_code=400,
                model=self.model,
            ) from cause

    model = BrokenModel(runtime, [])
    result = await runtime.explore(
        params(useVision=True, model={"apiKey": "diagnostic-model-secret"}), llm=model
    )
    assert not result["success"] and model.called.is_set()
    assert result["iterations"] == 3
    assert result["tokenUsage"]["totalTokens"] == 0
    assert "连续 3 次失败" in result["result"]
    assert "ModelProviderError (HTTP 400)" in result["result"]
    assert "Invalid max_tokens value, valid range [1, 393216]" in result["result"]
    assert "[REDACTED]" in result["result"]
    assert "diagnostic-model-secret" not in json.dumps(result)
    progress = [event for event in runtime.events if event.get("kind") == "progress"]
    assert progress and any(
        "失败" in event["data"].get("message", "") for event in progress
    )
    assert "diagnostic-model-secret" not in json.dumps(progress)
    assert runtime.browser is not None


@pytest.mark.asyncio
async def test_real_sdk_context_failure_survives_empty_history(runtime, monkeypatch):
    async def broken_context(agent, step_info=None):
        raise RuntimeError("fixture DOM context preparation failed")

    monkeypatch.setattr(PersistentAgent, "_prepare_context", broken_context)
    model = ScriptedModel(runtime, [])
    result = await runtime.explore(params(useVision=True, maxSteps=200), llm=model)
    assert not result["success"] and not model.called.is_set()
    assert result["iterations"] == 3
    assert result["tokenUsage"]["totalTokens"] == 0
    assert "RuntimeError: fixture DOM context preparation failed" in result["result"]
    assert "连续 3 次失败" in result["result"]
    assert "步数限制" not in result["result"]
    assert not any(event.get("kind") == "action_started" for event in runtime.events)


@pytest.mark.asyncio
async def test_real_sdk_startup_empty_assertion_is_reported_as_startup(
    runtime, monkeypatch
):
    async def broken_startup(agent):
        raise AssertionError()

    monkeypatch.setattr(PersistentAgent, "_log_agent_run", broken_startup)
    model = ScriptedModel(runtime, [])
    result = await runtime.explore(params(useVision=True), llm=model)
    assert not result["success"] and not model.called.is_set()
    assert result["iterations"] == 0
    assert "启动失败" in result["result"]
    assert "AssertionError" in result["result"]
    assert "步数限制" not in result["result"]


@pytest.mark.asyncio
async def test_driver_early_failure_keeps_exception_chain_and_redacts_request_secret():
    script = """
import asyncio
import driver
async def fail_before_explorer(self, params):
    try:
        raise ValueError('fixture initialize failed ' + params['model']['apiKey'])
    except ValueError as cause:
        raise AssertionError() from cause
driver.BrowserRuntime.start = fail_before_explorer
asyncio.run(driver.main())
"""
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-u",
        "-c",
        script,
        cwd=Path(__file__).parent,
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    secret = "fixture-request-secret-not-registered"
    try:
        process.stdin.write(
            (
                json.dumps(
                    {
                        "id": "failed-start",
                        "method": "start",
                        "params": {"model": {"apiKey": secret}},
                    }
                )
                + "\n"
            ).encode()
        )
        await process.stdin.drain()
        response = json.loads(await asyncio.wait_for(process.stdout.readline(), 10))
        assert response["ok"] is False and response["errorType"] == "AssertionError"
        assert "AssertionError" in response["error"]
        assert "ValueError: fixture initialize failed [REDACTED]" in response["error"]
        assert secret not in json.dumps(response)
    finally:
        process.stdin.close()
        await asyncio.wait_for(process.wait(), 10)
        assert secret.encode() not in await process.stderr.read()
