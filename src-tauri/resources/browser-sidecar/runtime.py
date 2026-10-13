"""一个 dispatcher session 对应一个共享的本地浏览器。"""

import asyncio
from pathlib import Path

import bootstrap  # noqa: F401
from browser_use import BrowserProfile
from browser_use.browser.events import (
    ClickCoordinateEvent,
    GoBackEvent,
    NavigateToUrlEvent,
    RefreshEvent,
)
from browser_use.browser.profile import ProxySettings

from dialogs import install_dialog_policy
from explorer import Explorer
from navigation import ManagedBrowser, install_navigation_policy
from previews import PreviewStream


class BrowserRuntime:
    def __init__(self, session_id, emit):
        self.session_id = session_id
        self.emit = emit
        self.browser = None
        self.explorer = None
        self.user_data_dir = None
        self.locale = None
        self.timezone = None
        self.url = None
        self.viewport = {"width": 1280, "height": 800}
        self.state = "closed"
        self.closing = False
        self.frames = None
        self.preview = PreviewStream(self)
        self.mutation_lock = asyncio.Lock()
        self.close_lock = asyncio.Lock()
        self.run_done = asyncio.Event()
        self.run_done.set()
        install_dialog_policy(self)
        install_navigation_policy(self)

    def log(self, message):
        self.emit({"event": "log", "sessionId": self.session_id, "message": message})

    def status(self, state=None, message=None):
        if state is not None:
            self.state = state
        value = {
            "sessionId": self.session_id,
            "state": self.state,
            "url": self.url,
            "message": message,
        }
        self.emit({"event": "status", "sessionId": self.session_id, "status": value})
        return value

    def agent_event(self, run_id, kind, data):
        # 密钥不能从上游错误或动作参数流入 UI/台账。
        self.emit(
            {
                "event": "agent",
                "sessionId": self.session_id,
                "runId": run_id,
                "kind": kind,
                "data": bootstrap.redact_value(data),
            }
        )

    def allow_dialog(self):
        return bool(
            self.explorer and not self.explorer.cancelled.is_set() and not self.closing
        )

    def dialog_event(self, kind, message, accepted):
        outcome = "确认" if accepted else "取消"
        preview = " ".join(message.split())[:500]
        data = {
            "message": f"自动弹窗处理：{kind}，{outcome}；页面提示：{preview}",
            "dialogType": kind,
            "accepted": accepted,
            "dialogMessage": message[:1000],
        }
        if self.explorer:
            self.agent_event(self.explorer.run_id, "progress", data)
        else:
            self.log(data["message"])

    def require_idle(self):
        if self.explorer is not None:
            raise RuntimeError("浏览器探索进行中，暂不允许手动修改页面")
        if self.closing:
            raise RuntimeError("浏览器正在关闭")

    def require_browser(self):
        if self.browser is None:
            raise RuntimeError("浏览器尚未启动")
        return self.browser

    async def start(self, params):
        async with self.mutation_lock:
            self.require_idle()
            if self.browser is not None:
                return self.status()
            raw_dir = params.get("userDataDir")
            if not raw_dir or not Path(raw_dir).is_absolute():
                raise ValueError("userDataDir 必须为绝对路径")
            self.user_data_dir = str(Path(raw_dir))
            Path(self.user_data_dir).mkdir(parents=True, exist_ok=True)
            self.locale = params.get("locale") or None
            self.timezone = params.get("timezone") or None
            self.viewport = params.get("viewport") or self.viewport
            proxy = params.get("proxy")
            if isinstance(proxy, str):
                proxy = {"server": proxy}
            self.closing = False
            self.status("starting", "正在启动 Browser Use 本地浏览器")
            self.browser = ManagedBrowser(
                browser_profile=BrowserProfile(
                    user_data_dir=self.user_data_dir,
                    profile_directory=params.get("profileDirectory") or "Default",
                    headless=bool(params.get("headless", True)),
                    viewport=self.viewport,
                    window_size=self.viewport,
                    proxy=ProxySettings(**proxy) if proxy else None,
                    downloads_path=str(Path(self.user_data_dir) / "downloads"),
                    keep_alive=True,
                    use_cloud=False,
                    enable_default_extensions=False,
                    captcha_solver=False,
                    permissions=[],
                    highlight_elements=False,
                    auto_download_pdfs=False,
                    args=[f"--lang={self.locale}"] if self.locale else [],
                )
            )
            self.browser._runtime = self
            try:
                await self.browser.start()
            except BaseException:
                await self.close_browser()
                raise
            self.url = await self.browser.get_current_page_url()
            self.frames = asyncio.create_task(self.preview.run())
            self.frames.add_done_callback(self._report_frames_exit)
            return self.status("ready")

    def _report_frames_exit(self, task):
        # 预览采集意外死亡只表现为画面不再刷新；留痕一条日志才可排查。
        if not task.cancelled() and task.exception() is not None:
            self.log(f"浏览器预览采集已停止：{type(task.exception()).__name__}")

    def set_preview(self, params):
        return self.preview.replace(params)

    async def dispatch(self, event):
        pending = self.require_browser().event_bus.dispatch(event)
        await pending
        return await pending.event_result(raise_if_any=True, raise_if_none=False)

    async def manual(self, method, params):
        async with self.mutation_lock:
            self.require_idle()
            browser = self.require_browser()
            if method == "open_url":
                await self.dispatch(NavigateToUrlEvent(url=str(params["url"])))
            elif method == "back":
                await self.dispatch(GoBackEvent())
            elif method == "reload":
                await self.dispatch(RefreshEvent())
            elif method == "click":
                await self.dispatch(
                    ClickCoordinateEvent(
                        coordinate_x=int(params["x"]), coordinate_y=int(params["y"])
                    )
                )
            else:
                raise ValueError(f"未知浏览器操作：{method}")
            self.url = await browser.get_current_page_url()
            return self.status("ready")

    async def screenshot(self):
        browser = self.require_browser()
        session = await browser.get_or_create_cdp_session(focus=False)
        result = await session.cdp_client.send.Page.captureScreenshot(
            params={"format": "jpeg", "quality": 65, "captureBeyondViewport": False},
            session_id=session.session_id,
        )
        return {"data": f"data:image/jpeg;base64,{result['data']}", **self.viewport}

    async def explore(self, params, llm=None):
        async with self.mutation_lock:
            self.require_idle()
            self.require_browser()
            if not str(params.get("runId", "")).strip():
                raise ValueError("runId 不能为空")
            self.explorer = Explorer(self, params, llm=llm)
            self.preview.notify()
            self.run_done.clear()
            self.status("busy", "浏览器探索中")
        try:
            return await self.explorer.execute()
        finally:
            self.explorer = None
            self.preview.notify()
            self.run_done.set()
            if self.browser is not None and not self.closing:
                self.url = await self.browser.get_current_page_url()
                self.status("ready")

    async def cancel(self, run_id):
        current = self.explorer
        if current is None:
            return {"cancelled": False, "runId": run_id}
        if current.run_id != run_id:
            raise ValueError("runId 与正在执行的探索不一致")
        self.closing = True
        self.preview.notify()
        current.cancel()
        await asyncio.wait_for(self.run_done.wait(), timeout=10)
        await self.close_browser()
        return {"cancelled": True, "runId": run_id}

    async def close_browser(self):
        async with self.close_lock:
            self.closing = True
            if self.frames:
                self.frames.cancel()
                await asyncio.gather(self.frames, return_exceptions=True)
                self.frames = None
            if self.browser is not None:
                # 失败必须向 Rust 返回，宿主仍拥有进程组回收的最终保障。
                await asyncio.wait_for(self.browser.kill(), timeout=8)
                self.browser = None
            self.url = None
            self.status("closed")
            self.closing = False

    async def close(self):
        if self.explorer:
            await self.cancel(self.explorer.run_id)
        await self.close_browser()
        return self.status()
