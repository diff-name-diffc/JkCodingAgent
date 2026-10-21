"""无需模型凭据的真实 Chrome/browser-use 集成回归。"""

import asyncio
import functools
import http.server
import json
import os
import sys
import threading
from pathlib import Path

import bootstrap  # noqa: F401
import pytest
import pytest_asyncio
from browser_use.llm.views import ChatInvokeCompletion, ChatInvokeUsage

from explorer import (
    ALLOWED_ACTIONS,
    ExplorationCancelled,
    ExplorationTimeout,
    ObservedTools,
)
from runtime import BrowserRuntime


@pytest.fixture
def site(tmp_path):
    (tmp_path / "index.html").write_text(
        '<html><body><h1>Browser fixture</h1><a href="/next.html">Next</a>'
        '<button id="confirm" onclick="document.querySelector(\'#answer\').textContent='
        "confirm('Continue?') ? 'accepted' : 'dismissed'\">Confirm</button>"
        '<p id="answer">pending</p></body></html>',
        encoding="utf-8",
    )
    (tmp_path / "next.html").write_text("<html><body><h1>Next page</h1></body></html>")
    handler = functools.partial(
        http.server.SimpleHTTPRequestHandler, directory=str(tmp_path)
    )
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{server.server_port}/index.html"
    server.shutdown()
    server.server_close()
    thread.join()


@pytest_asyncio.fixture
async def runtime(tmp_path):
    events = []
    instance = BrowserRuntime("test-session", events.append)
    instance.events = events
    await instance.start(
        {
            "userDataDir": str(tmp_path / "profile"),
            "headless": True,
            "locale": "en-US",
            "timezone": "UTC",
        }
    )
    yield instance
    await instance.close()


class ScriptedModel:
    model = "fixture-model"
    provider = "openai"
    name = "fixture-model"
    _verified_api_keys = True

    def __init__(self, runtime, actions, delay=0):
        self.runtime = runtime
        self.actions = iter(actions)
        self.delay = delay
        self.called = asyncio.Event()
        self.messages = []

    async def ainvoke(self, messages, output_format=None, **kwargs):
        self.messages = messages
        self.called.set()
        if self.delay:
            await asyncio.sleep(self.delay)
        action = next(
            self.actions, {"done": {"text": "fixture complete", "success": True}}
        )
        if action == "confirm":
            state = await self.runtime.browser.get_browser_state_summary(
                include_screenshot=False
            )
            index = next(
                index
                for index, node in state.dom_state.selector_map.items()
                if node.attributes.get("id") == "confirm"
            )
            action = {"click": {"index": index}}
        output = output_format.model_validate(
            {
                "evaluation_previous_goal": "Fixture observation",
                "memory": "fixture",
                "next_goal": "Execute fixture action",
                "action": [action],
            }
        )
        usage = ChatInvokeUsage(
            prompt_tokens=10,
            completion_tokens=3,
            total_tokens=13,
            prompt_cached_tokens=None,
            prompt_cache_creation_tokens=None,
            prompt_image_tokens=None,
        )
        return ChatInvokeCompletion(completion=output, usage=usage)


def params(**overrides):
    return {
        "runId": "test-run",
        "task": "Use the fixture browser",
        "maxSteps": 5,
        "timeoutSecs": 60,
        "model": {},
        **overrides,
    }


async def evaluate(runtime, expression):
    session = await runtime.browser.get_or_create_cdp_session(focus=False)
    result = await session.cdp_client.send.Runtime.evaluate(
        params={"expression": expression, "returnByValue": True},
        session_id=session.session_id,
    )
    return result["result"].get("value")


@pytest.mark.asyncio
async def test_navigation_actions_usage_and_reuse(runtime, site):
    result = await runtime.explore(
        params(),
        llm=ScriptedModel(
            runtime,
            [
                {"navigate": {"url": site}},
                "confirm",
            ],
        ),
    )
    assert result["success"] is True
    assert result["tokenUsage"]["promptTokens"] >= 20
    assert (
        await evaluate(runtime, "document.querySelector('#answer').textContent")
        == "accepted"
    )
    started = [e for e in runtime.events if e.get("kind") == "action_started"]
    finished = [e for e in runtime.events if e.get("kind") == "action_finished"]
    assert [e["data"]["taskId"] for e in started] == [
        e["data"]["taskId"] for e in finished
    ]
    assert any(
        e.get("kind") == "progress" and e["data"].get("accepted") is True
        for e in runtime.events
    )
    # 正常完成保留同一浏览器及事件总线，UI 和下一个 Agent 都能使用。
    browser = runtime.browser
    assert os.getpgid(browser._local_browser_watchdog.browser_pid) == os.getpgrp()
    await runtime.manual("reload", {})
    model = ScriptedModel(runtime, [])
    result = await runtime.explore(params(runId="second"), llm=model)
    assert result["success"] and runtime.browser is browser
    assert "image_url" not in json.dumps(
        [message.model_dump() for message in model.messages]
    )
    result = await runtime.explore(params(runId="vision", useVision=True), llm=model)
    assert result["success"]
    assert "image_url" in json.dumps(
        [message.model_dump() for message in model.messages]
    )
    assert (await runtime.screenshot())["data"].startswith("data:image/jpeg;base64,")


@pytest.mark.asyncio
async def test_manual_confirmation_is_dismissed(runtime, site):
    await runtime.manual("open_url", {"url": site})
    assert await evaluate(runtime, "confirm('manual confirmation')") is False


@pytest.mark.asyncio
async def test_preview_subscription_during_agent_does_not_change_execution(
    runtime, site
):
    await runtime.manual("open_url", {"url": site})
    assert not any(event.get("event") == "frame" for event in runtime.events)
    resume = asyncio.Event()
    frame_received = asyncio.Event()

    class PausedModel(ScriptedModel):
        async def ainvoke(self, *args, **kwargs):
            self.called.set()
            await resume.wait()
            return await super().ainvoke(*args, **kwargs)

    def emit(event):
        runtime.events.append(event)
        if event.get("event") == "frame":
            frame_received.set()

    runtime.emit = emit
    model = PausedModel(runtime, ["confirm"])
    task = asyncio.create_task(
        runtime.explore(params(toolCallId="preview-call"), llm=model)
    )
    try:
        await asyncio.wait_for(model.called.wait(), timeout=10)
        runtime.set_preview(
            {
                "revision": 1,
                "observers": [
                    {
                        "observerId": "card",
                        "toolCallId": "preview-call",
                        "leaseMs": 8000,
                    }
                ],
            }
        )
        await asyncio.wait_for(frame_received.wait(), timeout=5)
        frame = next(e for e in runtime.events if e.get("event") == "frame")
        assert frame["runId"] == "test-run"
        assert frame["toolCallId"] == "preview-call"
        assert not task.done()
        runtime.set_preview({"revision": 2, "observers": []})
        assert not await runtime.preview.capture()
        resume.set()
        result = await asyncio.wait_for(task, timeout=15)
        assert result["success"] is True
        assert (
            await evaluate(runtime, "document.querySelector('#answer').textContent")
            == "accepted"
        )
    finally:
        resume.set()
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("shared_roots", [False, True])
async def test_local_file_ui_and_agent_preserve_workspace_boundary(
    runtime, tmp_path, shared_roots
):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    session = tmp_path / "session"
    session.mkdir()

    def scope(run_id):
        if not shared_roots:
            return {"allowedFileRoot": str(workspace)}
        return {
            "allowedFileRoots": [str(session), str(workspace)],
            "temporaryDirectory": str(session / "browser-runs" / run_id),
        }

    page = workspace / "index.html"
    page.write_text(
        '<html><body><h1>Workspace fixture sentinel</h1><button id="confirm" '
        "onclick=\"this.textContent='Workspace clicked'\">Click local</button></body></html>"
    )
    outside = tmp_path / "outside.html"
    outside.write_text("<html><body>Outside private sentinel</body></html>")
    await runtime.manual("open_url", {"url": page.as_uri()})
    assert "Workspace fixture" in await evaluate(runtime, "document.body.innerText")
    await runtime.manual("open_url", {"url": "about:blank"})
    model = ScriptedModel(runtime, [{"navigate": {"url": page.as_uri()}}, "confirm"])
    result = await runtime.explore(params(**scope("first")), llm=model)
    assert result["success"]
    assert "Workspace fixture sentinel" in json.dumps(
        [message.model_dump() for message in model.messages]
    )
    assert "Workspace clicked" in await evaluate(runtime, "document.body.innerText")
    if shared_roots:
        assert Path(runtime.browser.browser_profile.downloads_path) == (
            session / "browser-runs" / "first" / "downloads"
        ).resolve()
        assert not (Path(runtime.user_data_dir) / "runs").exists()
    model = ScriptedModel(
        runtime,
        [
            {"navigate": {"url": outside.as_uri()}},
            {"done": {"text": "blocked", "success": False}},
        ],
    )
    result = await runtime.explore(
        params(runId="outside", **scope("outside")), llm=model
    )
    assert result["success"] is False
    assert await runtime.browser.get_current_page_url() == page.as_uri()
    assert "Outside private sentinel" not in json.dumps(
        [message.model_dump() for message in model.messages]
    )
    await runtime.manual("open_url", {"url": outside.as_uri()})
    model = ScriptedModel(runtime, [])
    result = await runtime.explore(
        params(runId="existing-outside", maxSteps=1, **scope("existing-outside")),
        llm=model,
    )
    assert result["success"] is False and not model.called.is_set()


@pytest.mark.asyncio
async def test_cancel_blocks_mutation_and_closes(runtime):
    model = ScriptedModel(runtime, [], delay=30)
    job = asyncio.create_task(runtime.explore(params(), llm=model))
    await asyncio.wait_for(model.called.wait(), timeout=30)
    with pytest.raises(RuntimeError, match="探索进行中"):
        await runtime.manual("reload", {})
    runtime.explorer.cancelled.set()
    assert await evaluate(runtime, "confirm('during cancellation')") is False
    await runtime.cancel("test-run")
    with pytest.raises(ExplorationCancelled):
        await job
    assert runtime.browser is None and not runtime.allow_dialog()


@pytest.mark.asyncio
async def test_timeout_is_error_and_closes(runtime):
    with pytest.raises(ExplorationTimeout):
        await runtime.explore(
            params(timeoutSecs=0.2), llm=ScriptedModel(runtime, [], delay=30)
        )
    assert runtime.browser is None


@pytest.mark.asyncio
async def test_step_budget_and_declared_failure_are_not_success(runtime):
    result = await runtime.explore(
        params(maxSteps=1),
        llm=ScriptedModel(
            runtime,
            [
                {"wait": {"seconds": 0}},
                {"done": {"text": "cannot finish", "success": False}},
            ],
        ),
    )
    assert result["success"] is False
    result = await runtime.explore(
        params(runId="failed"),
        llm=ScriptedModel(
            runtime,
            [
                {"done": {"text": "site unavailable", "success": False}},
            ],
        ),
    )
    assert result["success"] is False and "site unavailable" in result["result"]


def test_action_surface_is_allowlisted():
    tools = ObservedTools(None)
    assert set(tools.registry.registry.actions) <= ALLOWED_ACTIONS
    assert not {
        "evaluate",
        "write_file",
        "read_file",
        "replace_file",
        "upload_file",
    } & set(tools.registry.registry.actions)


@pytest.mark.asyncio
async def test_jsonl_driver_with_real_browser(tmp_path, site):
    command = [sys.executable, "-u"]
    if os.environ.get("JKC_TEST_UV_RUNTIME") == "1":
        command = [
            "uv",
            "run",
            "--no-project",
            "--python",
            "3.12",
            "--with",
            "browser-use==0.13.11",
            "python",
            "-u",
        ]
    command.append(str(Path(__file__).with_name("driver.py")))
    with (tmp_path / "driver-stderr.log").open("wb") as stderr:
        process = await asyncio.create_subprocess_exec(
            *command,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=stderr,
            env={**os.environ, "JKC_BROWSER_SESSION_ID": "protocol-test"},
        )

        async def call(method, params):
            process.stdin.write(
                (
                    json.dumps({"id": method, "method": method, "params": params})
                    + "\n"
                ).encode()
            )
            await process.stdin.drain()
            while line := await asyncio.wait_for(
                process.stdout.readline(), timeout=120
            ):
                message = json.loads(line)
                if message.get("id") == method:
                    assert message["ok"], message
                    return message["result"]
            raise AssertionError("sidecar 未返回响应即退出")

        try:
            started = await call(
                "start", {"userDataDir": str(tmp_path / "protocol-profile")}
            )
            assert (
                started["sessionId"] == "protocol-test" and started["state"] == "ready"
            )
            navigated = await call("open_url", {"url": site})
            assert navigated["url"] == site
            assert (await call("screenshot", {}))["data"].startswith(
                "data:image/jpeg;base64,"
            )
            assert (
                await call(
                    "set_preview",
                    {
                        "revision": 1,
                        "observers": [
                            {
                                "observerId": "protocol-viewer",
                                "toolCallId": None,
                                "leaseMs": 8000,
                            }
                        ],
                    },
                )
            )["accepted"]
            async with asyncio.timeout(5):
                while True:
                    frame = json.loads(await process.stdout.readline())
                    if frame.get("event") == "frame":
                        break
            assert frame["runId"] is None and frame["toolCallId"] is None
            assert frame["sequence"] == 1 and frame["capturedAt"] > 0
            assert frame["url"] == site
            await call("set_preview", {"revision": 2, "observers": []})
            assert not (await call("set_preview", {"revision": 1, "observers": []}))[
                "accepted"
            ]
            assert (await call("close", {}))["state"] == "closed"
        finally:
            process.stdin.close()
            await asyncio.wait_for(process.wait(), timeout=15)
        assert process.returncode == 0


def test_redaction_covers_split_log_writes():
    import io
    from bootstrap import RedactingStream, register_secret

    register_secret("fixture-api-key-never-log")
    output = io.StringIO()
    stream = RedactingStream(output)
    stream.write("failure fixture-api-")
    stream.write("key-never-log\n")
    stream.flush()
    assert "fixture-api-key-never-log" not in output.getvalue()
    assert "[REDACTED]" in output.getvalue()
