"""浏览探索的 URL/工作区文件边界，手动文件展示由 Rust 路径校验授权。"""

import asyncio
from pathlib import Path
from typing import ClassVar
from urllib.parse import unquote, urlsplit

from browser_use import Browser
from browser_use.browser.watchdogs.security_watchdog import SecurityWatchdog
from pydantic import PrivateAttr


def is_navigation_allowed(url, allowed_file_root=None):
    try:
        parsed = urlsplit(str(url).strip())
        if (parsed.scheme in {"http", "https"} and bool(parsed.hostname)) or (
            parsed.scheme == "about" and parsed.path == "blank"
        ):
            return True
        if (
            parsed.scheme != "file"
            or parsed.netloc.lower() not in {"", "localhost"}
            or not allowed_file_root
        ):
            return False
        root = Path(allowed_file_root)
        path = Path(unquote(parsed.path, errors="strict"))
        if not root.is_absolute() or not path.is_absolute():
            return False
        path.resolve().relative_to(root.resolve())
        return True
    except (ValueError, OSError, RuntimeError):
        return False


def require_navigation_allowed(url, allowed_file_root=None):
    if not is_navigation_allowed(url, allowed_file_root):
        raise ValueError("浏览器探索仅允许 http(s)、about:blank 和工作区内本地文件")


class ManagedBrowser(Browser):
    _runtime = PrivateAttr(default=None)

    async def get_browser_state_summary(
        self, include_screenshot=True, cached=False, include_recent_events=False
    ):
        exploring = self._runtime is not None and self._runtime.explorer is not None
        root = (
            self._runtime.explorer.params.get("allowedFileRoot") if exploring else None
        )
        if exploring:
            # 页面可能来自 UI、历史后退或新标签，感知前仍须校验。
            require_navigation_allowed(await self.get_current_page_url(), root)
        state = await super().get_browser_state_summary(
            include_screenshot, cached, include_recent_events
        )
        if exploring:
            require_navigation_allowed(state.url, root)
            if urlsplit(state.url).scheme == "file":
                # 上游将非 http(s) 视作空白页；工作区文件复用同一 DOM/截图构建器。
                async with asyncio.timeout(15):
                    state.dom_state = (
                        await self._dom_watchdog._build_dom_tree_without_highlights()
                    )
                    if include_screenshot:
                        state.screenshot = (
                            await self._dom_watchdog._capture_clean_screenshot()
                        )
                state.title = Path(unquote(urlsplit(state.url).path)).name
                self._cached_browser_state_summary = state
            require_navigation_allowed(await self.get_current_page_url(), root)
        return state


class ManagedSecurityWatchdog(SecurityWatchdog):
    policy: ClassVar = None

    def _is_url_allowed(self, url):
        if type(self).policy.explorer is not None:
            return is_navigation_allowed(
                url, type(self).policy.explorer.params.get("allowedFileRoot")
            )
        # 此时只能由宿主 UI 导航，file 路径已经在 Rust 命令入口校验。
        if str(url).lower().startswith("file:"):
            return True
        return super()._is_url_allowed(url)


def install_navigation_policy(policy):
    import browser_use.browser.watchdogs.security_watchdog as module

    ManagedSecurityWatchdog.policy = policy
    module.SecurityWatchdog = ManagedSecurityWatchdog
