"""可见观察者租约驱动的单路预览；不参与 Agent 动作和模型上下文。"""

import asyncio
import math
import time

CAPTURE_TIMEOUT_SECONDS = 3


class PreviewStream:
    def __init__(self, runtime, *, monotonic=time.monotonic, wall_clock=time.time):
        self.runtime = runtime
        self.monotonic = monotonic
        self.wall_clock = wall_clock
        self.revision = -1
        self.observers = {}
        self.epoch = 0
        self.sequence = 0
        self.changed = asyncio.Event()

    def notify(self):
        self.changed.set()

    def replace(self, params):
        revision = params.get("revision")
        if type(revision) is not int or not 0 <= revision <= 2**64 - 1:
            raise ValueError("预览 revision 必须为 u64")
        if revision <= self.revision:
            return {"revision": self.revision, "accepted": False}
        values = params.get("observers")
        if not isinstance(values, list):
            raise ValueError("预览 observers 必须是数组")
        now = self.monotonic()
        observers = {}
        for value in values:
            if not isinstance(value, dict):
                raise ValueError("预览 observer 必须是对象")
            observer_id = value.get("observerId")
            tool_call_id = value.get("toolCallId")
            lease_ms = value.get("leaseMs")
            if not isinstance(observer_id, str) or not observer_id.strip():
                raise ValueError("预览 observerId 不能为空")
            if observer_id in observers:
                raise ValueError("预览 observerId 重复")
            if tool_call_id is not None and (
                not isinstance(tool_call_id, str) or not tool_call_id.strip()
            ):
                raise ValueError("预览 toolCallId 必须为空或非空字符串")
            if (
                type(lease_ms) not in (int, float)
                or not math.isfinite(lease_ms)
                or lease_ms <= 0
            ):
                raise ValueError("预览 leaseMs 必须为正有限数值")
            observers[observer_id] = (tool_call_id, now + min(lease_ms, 8000) / 1000)
        previous = {
            key: scope
            for key, (scope, expiry) in self.observers.items()
            if expiry > now
        }
        current = {key: scope for key, (scope, _) in observers.items()}
        if previous != current:
            # 隐藏后重新显示也不能接收隐藏前已经在途的截图；单纯续租不失效。
            self.epoch += 1
        self.observers = observers
        self.revision = revision
        self.notify()
        return {"revision": revision, "accepted": True}

    def context(self):
        runtime = self.runtime
        if runtime.browser is None or runtime.closing:
            return None
        explorer = runtime.explorer
        if explorer is not None and explorer.cancelled.is_set():
            return None
        return (
            runtime.browser,
            explorer,
            explorer.run_id if explorer else None,
            explorer.params.get("toolCallId") if explorer else None,
            self.epoch,
        )

    def observed(self, context):
        if context is None:
            return False
        now = self.monotonic()
        return any(
            expiry > now and (scope is None or scope == context[3])
            for scope, expiry in self.observers.values()
        )

    def current(self, context):
        current = self.context()
        return bool(
            current is not None
            and context[0] is current[0]
            and context[1] is current[1]
            and context[2:] == current[2:]
            and self.observed(context)
        )

    def report_error(self, context, error):
        if not self.current(context):
            return
        message = f"浏览器预览采集失败：{type(error).__name__}"
        self.runtime.emit(
            {
                "event": "preview_error",
                "sessionId": self.runtime.session_id,
                "runId": context[2],
                "toolCallId": context[3],
                "message": message,
            }
        )
        self.runtime.log(message)

    async def capture(self):
        context = self.context()
        if not self.observed(context):
            return False
        try:
            # URL 获取同样走 CDP，必须与截图共用预算，不能让预览任务挂死。
            async with asyncio.timeout(CAPTURE_TIMEOUT_SECONDS):
                frame = await self.runtime.screenshot()
                captured_at = int(self.wall_clock() * 1000)
                url = await context[0].get_current_page_url()
        except asyncio.CancelledError:
            raise
        except Exception as error:
            self.report_error(context, error)
            return False
        # 所有 await 完成后再检查，身份或可见性变化期间的截图不可进入 UI。
        if not self.current(context):
            return False
        self.sequence += 1
        self.runtime.emit(
            {
                "event": "frame",
                "sessionId": self.runtime.session_id,
                "runId": context[2],
                "toolCallId": context[3],
                "sequence": self.sequence,
                "capturedAt": captured_at,
                "url": url,
                **frame,
            }
        )
        if url != self.runtime.url:
            self.runtime.url = url
            self.runtime.status()
        return True

    async def run(self):
        next_capture = 0.0
        while self.runtime.browser is not None and not self.runtime.closing:
            self.changed.clear()
            if not self.observed(self.context()):
                await self.changed.wait()
                continue
            delay = next_capture - self.monotonic()
            if delay > 0:
                try:
                    await asyncio.wait_for(self.changed.wait(), timeout=delay)
                except TimeoutError:
                    pass
                continue
            await self.capture()
            # 续租/打开第二个观察窗口只唤醒条件检查，不能绕过单路限速。
            next_capture = self.monotonic() + 1
