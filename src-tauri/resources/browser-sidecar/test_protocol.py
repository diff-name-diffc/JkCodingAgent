"""SDK 请求和事件边界：不访问外部模型服务。"""

import asyncio
import io
import json
from types import SimpleNamespace

import bootstrap
import httpx
import pytest
from browser_use.llm.messages import SystemMessage, UserMessage
from browser_use import Tools
from browser_use.agent.views import ActionResult
from pydantic import BaseModel

from dialogs import handle_dialog
from explorer import ObservedTools, exploration_directory, model_from_config
from navigation import allowed_file_roots, is_navigation_allowed


class Output(BaseModel):
    answer: str


@pytest.mark.asyncio
@pytest.mark.parametrize("max_tokens", [None, 1234])
async def test_actual_chat_openai_request_and_usage(max_tokens):
    requests = []

    async def handler(request):
        requests.append(request)
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
                        "message": {
                            "role": "assistant",
                            "content": '{"answer":"ready"}',
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": 11,
                    "completion_tokens": 3,
                    "total_tokens": 14,
                },
            },
        )

    model = model_from_config(
        {
            "model": "fixture-model",
            "apiBase": "http://fixture.invalid/v1/chat/completions/",
            "apiKey": "fixture-not-real",
            "maxTokens": max_tokens,
            "temperature": 0.4,
        }
    )
    async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
        model.http_client = client
        result = await model.ainvoke(
            [SystemMessage(content="Return JSON"), UserMessage(content="Ready?")],
            output_format=Output,
        )
    assert result.completion.answer == "ready"
    assert result.usage.total_tokens == 14
    assert len(requests) == 1
    request = requests[0]
    body = json.loads(request.content)
    assert str(request.url) == "http://fixture.invalid/v1/chat/completions"
    assert body["temperature"] == 0.4
    assert body["response_format"] == {"type": "json_object"}
    assert "<json_schema>" in body["messages"][0]["content"]
    assert "frequency_penalty" not in body
    assert "reasoning_effort" not in body
    assert "max_completion_tokens" not in body
    if max_tokens is None:
        assert "max_tokens" not in body
    else:
        assert body["max_tokens"] == max_tokens


def test_secret_redaction_handles_escaped_values_and_flush():
    secret = 'fixture-key-"quote"-\\slash'
    bootstrap.register_secret(secret)
    value = {"arguments": {"text": secret}, "error": f"failed: {secret}"}
    assert secret not in json.dumps(bootstrap.redact_value(value))
    assert bootstrap.redact_value(value)["arguments"]["text"] == "[REDACTED]"
    output = io.StringIO()
    stream = bootstrap.RedactingStream(output)
    stream.write("failure " + secret[:10])
    stream.flush()
    assert output.getvalue() == "failure "
    stream.write(secret[10:] + "\n")
    stream.flush()
    assert output.getvalue() == "failure [REDACTED]\n"


@pytest.mark.asyncio
async def test_dialog_cancellation_has_no_scheduled_accept_window():
    allowed = True
    sent = []
    events = []

    async def send(*, params, session_id):
        sent.append((params["accept"], allowed))
        await asyncio.sleep(0)

    def stop():
        nonlocal allowed
        allowed = False

    root = SimpleNamespace(
        send=SimpleNamespace(Page=SimpleNamespace(handleJavaScriptDialog=send))
    )
    policy = SimpleNamespace(
        allow_dialog=lambda: allowed,
        dialog_event=lambda *event: events.append(event),
        log=lambda message: pytest.fail(message),
    )
    # wait_for(send(...)) 会先调度 stop 再发送旧 accept=True；直接 await 无此窗口。
    asyncio.get_running_loop().call_soon(stop)
    await handle_dialog(root, policy, {"type": "confirm", "message": "Submit?"}, "tab")
    await handle_dialog(root, policy, {"type": "confirm", "message": "Stopped"}, "tab")
    assert sent == [(True, True), (False, False)]
    assert events == [("confirm", "Submit?", True), ("confirm", "Stopped", False)]


def test_file_scope_decodes_traversal_and_resolves_symlinks(tmp_path):
    root = tmp_path / "workspace"
    root.mkdir()
    outside = tmp_path / "secret.html"
    outside.write_text("outside")
    (root / "escape.html").symlink_to(outside)
    for url in [
        (root / "index.html").as_uri(),
        f"file://localhost{root}/index.html",
        "https://example.com/",
        "http://127.0.0.1:8000/",
        "about:blank",
    ]:
        assert is_navigation_allowed(url, root), url
    for url in [
        outside.as_uri(),
        f"file://localhost{outside}",
        f"{root.as_uri()}/../secret.html",
        f"{root.as_uri()}/%2e%2e/secret.html",
        (root / "escape.html").as_uri(),
        f"file://remote{root}/index.html",
        "javascript:alert(1)",
        "chrome://version/",
        "data:text/html,hello",
        "blob:https://example.com/id",
    ]:
        assert not is_navigation_allowed(url, root), url
    assert not is_navigation_allowed((root / "index.html").as_uri())


def test_dependency_version_mismatch_fails_explicitly(monkeypatch):
    monkeypatch.setattr(bootstrap, "version", lambda package: "0.13.10")
    with pytest.raises(RuntimeError, match=r"0\.13\.11.*0\.13\.10"):
        bootstrap.verify_dependency()


def test_multiple_file_roots_share_projects_without_exposing_other_sessions(tmp_path):
    session = tmp_path / "session"
    project_a = tmp_path / "project a"
    project_b = tmp_path / "project-b"
    for root in [session, project_a, project_b]:
        root.mkdir()
    outside = tmp_path / "other-session" / "secret.html"
    outside.parent.mkdir()
    outside.write_text("private")
    (project_a / "escape.html").symlink_to(outside)
    (session / "shared.html").symlink_to(project_b / "index.html")
    roots = allowed_file_roots(
        {
            "allowedFileRoots": [str(session), str(project_a), str(project_b)],
            "allowedFileRoot": str(tmp_path),
        }
    )
    for root in [session, project_a, project_b]:
        assert is_navigation_allowed((root / "index.html").as_uri(), roots)
    assert is_navigation_allowed((session / "shared.html").as_uri(), roots)
    for url in [
        outside.as_uri(),
        (project_a / "escape.html").as_uri(),
        f"{session.as_uri()}/%2e%2e/other-session/secret.html",
        (tmp_path / "project-b-sibling" / "index.html").as_uri(),
    ]:
        assert not is_navigation_allowed(url, roots), url


def test_new_file_root_list_never_falls_back_to_legacy_authority(tmp_path):
    url = (tmp_path / "index.html").as_uri()
    assert is_navigation_allowed(url, allowed_file_roots({"allowedFileRoot": str(tmp_path)}))
    assert not is_navigation_allowed(
        url,
        allowed_file_roots({"allowedFileRoots": [], "allowedFileRoot": str(tmp_path)}),
    )
    for invalid in [None, str(tmp_path), ["relative"], [None]]:
        with pytest.raises(ValueError):
            allowed_file_roots({"allowedFileRoots": invalid})


def test_host_temporary_directory_stays_in_session_and_preserves_project_default(tmp_path):
    session = tmp_path / "session"
    project = tmp_path / "project"
    session.mkdir()
    project.mkdir()
    (session / "escape").symlink_to(project, target_is_directory=True)
    roots = {"allowedFileRoots": [str(session), str(project)]}
    temporary = session / ".jkcodingagent" / "browser-runs" / "run-a"
    assert exploration_directory(
        tmp_path / "profile", {**roots, "temporaryDirectory": str(temporary)}
    ) == temporary.resolve()
    for directory in [project / "run", session / "escape" / "run", session, "relative"]:
        with pytest.raises(ValueError):
            exploration_directory(
                tmp_path / "profile", {**roots, "temporaryDirectory": str(directory)}
            )
    with pytest.raises(ValueError):
        exploration_directory(
            tmp_path / "profile",
            {"allowedFileRoot": str(session), "temporaryDirectory": str(temporary)},
        )
    assert exploration_directory(
        tmp_path / "profile", {"allowedFileRoot": str(project)}
    ).parent == tmp_path / "profile" / "runs"


@pytest.mark.asyncio
@pytest.mark.parametrize("action_name", ["go_back", "switch"])
async def test_indirect_navigation_rechecks_destination(
    tmp_path, monkeypatch, action_name
):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    current_url = (workspace / "index.html").as_uri()

    async def page_url():
        return current_url

    async def navigate(*args, **kwargs):
        nonlocal current_url
        current_url = (tmp_path / "other-session" / "private.html").as_uri()
        return ActionResult(extracted_content="navigated")

    monkeypatch.setattr(Tools, "act", navigate)
    events = []
    owner = SimpleNamespace(
        params={"allowedFileRoots": [str(workspace)]},
        cancelled=asyncio.Event(),
        action_count=0,
        run_id="indirect-navigation",
        emit=lambda kind, **data: events.append((kind, data)),
        safe=str,
    )
    result = await ObservedTools(owner).act(
        SimpleNamespace(model_dump=lambda **kwargs: {action_name: {}}),
        SimpleNamespace(get_current_page_url=page_url),
    )
    assert "授权目录" in result.error
    assert events[-1][1]["success"] is False
