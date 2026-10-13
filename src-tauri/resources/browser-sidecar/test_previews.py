"""预览租约与在途帧的确定性回归；不依赖 Chrome 或模型。"""

import asyncio
from types import SimpleNamespace

import pytest

from previews import PreviewStream


class Clock:
    value = 10.0

    def __call__(self):
        return self.value


class FakeRuntime:
    def __init__(self):
        self.browser = self
        self.explorer = None
        self.closing = False
        self.session_id = "session"
        self.url = "http://localhost/page"
        self.events = []
        self.logs = []
        self.calls = 0
        self.capture_started = asyncio.Event()
        self.release_capture = asyncio.Event()
        self.release_capture.set()
        self.release_url = asyncio.Event()
        self.release_url.set()
        self.url_calls = 0
        self.fail = False

    async def screenshot(self):
        self.calls += 1
        self.capture_started.set()
        await self.release_capture.wait()
        if self.fail:
            raise RuntimeError("preview-only failure")
        return {"data": "data:image/jpeg;base64,fixture", "width": 1280, "height": 800}

    async def get_current_page_url(self):
        self.url_calls += 1
        await self.release_url.wait()
        return self.url

    def emit(self, event):
        self.events.append(event)

    def log(self, message):
        self.logs.append(message)

    def status(self):
        pass


def observer(observer_id="viewer", tool_call_id=None, lease_ms=8000):
    return {
        "observerId": observer_id,
        "toolCallId": tool_call_id,
        "leaseMs": lease_ms,
    }


def subscribe(stream, revision=1, observers=None):
    return stream.replace(
        {
            "revision": revision,
            "observers": [observer()] if observers is None else observers,
        }
    )


def exploring(runtime, run_id="run", tool_call_id="call"):
    runtime.explorer = SimpleNamespace(
        run_id=run_id,
        params={"toolCallId": tool_call_id},
        cancelled=asyncio.Event(),
    )


@pytest.fixture
def setup():
    runtime = FakeRuntime()
    clock = Clock()
    stream = PreviewStream(runtime, monotonic=clock, wall_clock=lambda: 123.456)
    return runtime, stream, clock


@pytest.mark.asyncio
async def test_default_and_unmatched_observers_do_not_capture(setup):
    runtime, stream, _ = setup
    assert not await stream.capture()
    subscribe(stream, observers=[observer(tool_call_id="call")])
    assert not await stream.capture()
    exploring(runtime, tool_call_id="other-call")
    assert not await stream.capture()
    assert runtime.calls == 0
    exploring(runtime)
    assert await stream.capture()
    assert runtime.calls == 1


@pytest.mark.asyncio
async def test_observers_share_one_frame_with_identity_and_sequence(setup):
    runtime, stream, _ = setup
    exploring(runtime)
    subscribe(stream, observers=[observer(), observer("card", "call")])
    assert await stream.capture()
    assert runtime.calls == 1
    frame = runtime.events[0]
    assert {
        key: frame[key]
        for key in ("sessionId", "runId", "toolCallId", "sequence", "capturedAt", "url")
    } == {
        "sessionId": "session",
        "runId": "run",
        "toolCallId": "call",
        "sequence": 1,
        "capturedAt": 123456,
        "url": runtime.url,
    }
    runtime.explorer = None
    assert await stream.capture()
    assert runtime.events[-1]["runId"] is None
    assert runtime.events[-1]["toolCallId"] is None
    assert runtime.events[-1]["sequence"] == 2
    subscribe(stream, 2, [observer("card", "call")])
    assert not await stream.capture()


def test_revision_renewal_and_eight_second_lease_cap(setup):
    _, stream, clock = setup
    subscribe(stream, 3, [observer(lease_ms=999999)])
    assert stream.observers["viewer"][1] == 18
    epoch = stream.epoch
    clock.value = 12
    subscribe(stream, 4)
    assert stream.epoch == epoch
    assert stream.observers["viewer"][1] == 20
    assert subscribe(stream, 3, []) == {"revision": 4, "accepted": False}
    assert stream.observed(stream.context())
    clock.value = 20
    assert not stream.observed(stream.context())
    subscribe(stream, 5)
    assert stream.epoch == epoch + 1


@pytest.mark.parametrize(
    "params",
    [
        {"revision": True, "observers": []},
        {"revision": -1, "observers": []},
        {"revision": 2**64, "observers": []},
        {"revision": 1, "observers": {}},
        {"revision": 1, "observers": [observer(), observer()]},
        {"revision": 1, "observers": [observer(tool_call_id="")]},
        {"revision": 1, "observers": [observer(lease_ms=0)]},
        {"revision": 1, "observers": [observer(lease_ms=float("nan"))]},
    ],
)
def test_invalid_snapshot_does_not_mutate_subscriptions(setup, params):
    _, stream, _ = setup
    with pytest.raises(ValueError):
        stream.replace(params)
    assert stream.revision == -1
    assert not stream.observers


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "boundary", ["hide", "expire", "next_run", "finished", "cancel", "close"]
)
async def test_inflight_frame_dropped_at_visibility_and_run_boundaries(setup, boundary):
    runtime, stream, clock = setup
    exploring(runtime)
    subscribe(stream)
    runtime.release_capture.clear()
    task = asyncio.create_task(stream.capture())
    await runtime.capture_started.wait()
    if boundary == "hide":
        subscribe(stream, 2, [])
        subscribe(stream, 3)
    elif boundary == "expire":
        clock.value += 8
    elif boundary == "next_run":
        exploring(runtime, "next-run", "next-call")
    elif boundary == "finished":
        runtime.explorer = None
    elif boundary == "cancel":
        runtime.explorer.cancelled.set()
    else:
        runtime.closing = True
    runtime.release_capture.set()
    assert not await task
    assert not runtime.events


@pytest.mark.asyncio
async def test_live_renewal_preserves_inflight_frame(setup):
    runtime, stream, clock = setup
    subscribe(stream)
    runtime.release_capture.clear()
    task = asyncio.create_task(stream.capture())
    await runtime.capture_started.wait()
    clock.value += 1
    subscribe(stream, 2)
    runtime.release_capture.set()
    assert await task


@pytest.mark.asyncio
async def test_scheduler_waits_when_hidden_and_renewal_does_not_bypass_limit(setup):
    runtime, stream, clock = setup
    task = asyncio.create_task(stream.run())
    try:
        await asyncio.sleep(0)
        assert runtime.calls == 0
        subscribe(stream)
        await runtime.capture_started.wait()
        for _ in range(10):
            await asyncio.sleep(0)
        assert len(runtime.events) == 1
        for revision in range(2, 8):
            subscribe(stream, revision)
            for _ in range(4):
                await asyncio.sleep(0)
        assert runtime.calls == 1
        clock.value += 1
        stream.notify()
        for _ in range(15):
            await asyncio.sleep(0)
        assert runtime.calls == 2
        subscribe(stream, 8, [])
        clock.value += 20
        stream.notify()
        for _ in range(10):
            await asyncio.sleep(0)
        assert runtime.calls == 2
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_capture_failure_isolated_from_explorer(setup):
    runtime, stream, _ = setup
    exploring(runtime)
    explorer = runtime.explorer
    runtime.fail = True
    subscribe(stream)
    task = asyncio.create_task(stream.run())
    try:
        await runtime.capture_started.wait()
        for _ in range(10):
            await asyncio.sleep(0)
        assert runtime.logs == ["浏览器预览采集失败：RuntimeError"]
        assert runtime.events == [
            {
                "event": "preview_error",
                "sessionId": "session",
                "runId": "run",
                "toolCallId": "call",
                "message": "浏览器预览采集失败：RuntimeError",
            }
        ]
        assert runtime.explorer is explorer
        assert not explorer.cancelled.is_set()
        assert not runtime.closing
        assert not task.done()
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["screenshot", "url"])
async def test_capture_budget_covers_both_screenshot_and_url(setup, monkeypatch, phase):
    import previews

    runtime, stream, _ = setup
    exploring(runtime)
    subscribe(stream)
    # timeout(0) 在首次挂起时取消，不依赖真实时间或轮询。
    monkeypatch.setattr(previews, "CAPTURE_TIMEOUT_SECONDS", 0)
    if phase == "screenshot":
        runtime.release_capture.clear()
    else:
        runtime.release_url.clear()
    assert not await stream.capture()
    assert runtime.calls == 1
    assert runtime.url_calls == (1 if phase == "url" else 0)
    assert runtime.events[0]["event"] == "preview_error"
    assert runtime.events[0]["message"] == "浏览器预览采集失败：TimeoutError"
    assert runtime.events[0]["runId"] == "run"
    assert runtime.events[0]["toolCallId"] == "call"
    assert not runtime.explorer.cancelled.is_set()


@pytest.mark.asyncio
@pytest.mark.parametrize("boundary", ["hide", "expire", "next_run", "cancel", "close"])
async def test_late_capture_errors_do_not_reach_hidden_or_different_runs(
    setup, boundary
):
    runtime, stream, clock = setup
    exploring(runtime)
    subscribe(stream)
    runtime.fail = True
    runtime.release_capture.clear()
    task = asyncio.create_task(stream.capture())
    await runtime.capture_started.wait()
    if boundary == "hide":
        subscribe(stream, 2, [])
        subscribe(stream, 3)
    elif boundary == "expire":
        clock.value += 8
    elif boundary == "next_run":
        exploring(runtime, "next-run", "next-call")
    elif boundary == "cancel":
        runtime.explorer.cancelled.set()
    else:
        runtime.closing = True
    runtime.release_capture.set()
    assert not await task
    assert not runtime.events
    assert not runtime.logs
