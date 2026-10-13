"""替换固定版本 browser-use 的自动弹窗策略，取消后不再确认。"""

import asyncio
from typing import ClassVar

from browser_use.browser.events import TabCreatedEvent
from browser_use.browser.watchdog_base import BaseWatchdog
from pydantic import PrivateAttr


async def handle_dialog(root, policy, data, session_id=None):
    if session_id is None:
        policy.log("弹窗缺少 CDP sessionId，拒绝自动处理")
        return
    kind = data.get("type", "alert")
    try:
        async with asyncio.timeout(2):
            # 此处到开始发送间不能创建新任务/让出循环；已开始发送的命令无法撤回。
            accept = policy.allow_dialog() and kind in {
                "alert",
                "confirm",
                "beforeunload",
            }
            policy.dialog_event(kind, str(data.get("message", "")), accept)
            await root.send.Page.handleJavaScriptDialog(
                params={"accept": accept}, session_id=session_id
            )
    except Exception as error:
        policy.log(f"弹窗处理失败：{type(error).__name__}")


class ManagedPopupsWatchdog(BaseWatchdog):
    LISTENS_TO: ClassVar = [TabCreatedEvent]
    EMITS: ClassVar = []
    policy: ClassVar = None
    _registered: bool = PrivateAttr(default=False)

    async def on_TabCreatedEvent(self, event: TabCreatedEvent) -> None:
        session = await self.browser_session.get_or_create_cdp_session(
            target_id=event.target_id, focus=False
        )
        await session.cdp_client.send.Page.enable(session_id=session.session_id)
        policy = type(self).policy
        if policy.locale:
            await session.cdp_client.send.Emulation.setLocaleOverride(
                params={"locale": policy.locale}, session_id=session.session_id
            )
        if policy.timezone:
            await session.cdp_client.send.Emulation.setTimezoneOverride(
                params={"timezoneId": policy.timezone}, session_id=session.session_id
            )
        if self._registered:
            return
        self._registered = True
        root = self.browser_session.cdp_client

        async def on_dialog(data, session_id=None):
            await handle_dialog(root, policy, data, session_id)

        root.register.Page.javascriptDialogOpening(on_dialog)


def install_dialog_policy(policy) -> None:
    # 上游在 attach_all_watchdogs 内延迟导入该类；固定依赖版本并以集成测试守护。
    import browser_use.browser.watchdogs.popups_watchdog as module

    ManagedPopupsWatchdog.policy = policy
    module.PopupsWatchdog = ManagedPopupsWatchdog
