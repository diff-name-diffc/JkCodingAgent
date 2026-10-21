"""浏览探索的 URL/工作区文件边界，手动文件展示由 Rust 路径校验授权。"""

import asyncio
from pathlib import Path
from typing import ClassVar
from urllib.parse import unquote, urlsplit

from browser_use import Browser
from browser_use.browser.watchdogs.security_watchdog import SecurityWatchdog
from pydantic import PrivateAttr


def allowed_file_roots(params):
    """宿主多根授权优先；旧项目调用继续使用单根，空多根不得回退扩大权限。"""
    if "allowedFileRoots" in params:
        roots = params["allowedFileRoots"]
        if not isinstance(roots, list):
            raise ValueError("allowedFileRoots 必须为绝对目录列表")
    else:
        root = params.get("allowedFileRoot")
        roots = [root] if root else []
    if any(not isinstance(root, str) or not Path(root).is_absolute() for root in roots):
        raise ValueError("浏览器授权目录必须为绝对路径")
    return tuple(Path(root) for root in roots)


def is_navigation_allowed(url, allowed_roots=()):
    try:
        parsed = urlsplit(str(url).strip())
        if (parsed.scheme in {"http", "https"} and bool(parsed.hostname)) or (
            parsed.scheme == "about" and parsed.path == "blank"
        ):
            return True
        if (
            parsed.scheme != "file"
            or parsed.netloc.lower() not in {"", "localhost"}
            or not allowed_roots
        ):
            return False
        roots = (
            (Path(allowed_roots),)
            if isinstance(allowed_roots, (str, Path))
            else tuple(Path(root) for root in allowed_roots)
        )
        path = Path(unquote(parsed.path, errors="strict"))
        if not all(root.is_absolute() for root in roots) or not path.is_absolute():
            return False
        path = path.resolve()
        return any(path.is_relative_to(root.resolve()) for root in roots)
    except (ValueError, OSError, RuntimeError, TypeError):
        return False


def require_navigation_allowed(url, allowed_roots=()):
    if not is_navigation_allowed(url, allowed_roots):
        raise ValueError("浏览器探索仅允许 http(s)、about:blank 和授权目录内本地文件")


class ManagedBrowser(Browser):
    _runtime = PrivateAttr(default=None)

    async def get_browser_state_summary(
        self, include_screenshot=True, cached=False, include_recent_events=False
    ):
        exploring = self._runtime is not None and self._runtime.explorer is not None
        roots = (
            allowed_file_roots(self._runtime.explorer.params) if exploring else ()
        )
        if exploring:
            # 页面可能来自 UI、历史后退或新标签，感知前仍须校验。
            require_navigation_allowed(await self.get_current_page_url(), roots)
        state = await super().get_browser_state_summary(
            include_screenshot, cached, include_recent_events
        )
        if exploring:
            require_navigation_allowed(state.url, roots)
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
            require_navigation_allowed(await self.get_current_page_url(), roots)
        return state


class ManagedSecurityWatchdog(SecurityWatchdog):
    policy: ClassVar = None

    def _is_url_allowed(self, url):
        if type(self).policy.explorer is not None:
            return is_navigation_allowed(
                url, allowed_file_roots(type(self).policy.explorer.params)
            )
        # 此时只能由宿主 UI 导航，file 路径已经在 Rust 命令入口校验。
        if str(url).lower().startswith("file:"):
            return True
        return super()._is_url_allowed(url)


def install_navigation_policy(policy):
    import browser_use.browser.watchdogs.security_watchdog as module

    ManagedSecurityWatchdog.policy = policy
    module.SecurityWatchdog = ManagedSecurityWatchdog
