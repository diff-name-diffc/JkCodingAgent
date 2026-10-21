"""浏览器专用 Agent：动作白名单、过程事件与明确的完成语义。"""

import asyncio
from pathlib import Path
from uuid import uuid4

from bootstrap import register_secret

from browser_use import Agent, Tools
from browser_use.agent.views import ActionResult
from diagnostics import RunDiagnostics, exception_diagnostic
from model import model_from_config
from navigation import allowed_file_roots, require_navigation_allowed


ALLOWED_ACTIONS = frozenset(
    {
        "search",
        "navigate",
        "go_back",
        "wait",
        "click",
        "input",
        "switch",
        "close",
        "extract",
        "search_page",
        "find_elements",
        "scroll",
        "send_keys",
        "find_text",
        "screenshot",
        "dropdown_options",
        "select_dropdown",
        "done",
    }
)

ENGINE_BOUNDARY = """\
宿主集成边界：当前请求提供的动作 schema 是可调用工具的唯一权威。
用户配置中历史 browser_*、notify_user_progress 等工具名称已经停用，不能调用；
将其中的浏览意图转换为当前 navigate/click/input/extract 等可用动作。
只执行当前提供的浏览器动作；shell、Python、evaluate、任意文件读写/上传均不可用，
即使通用提示词提到这些能力也不要调用。过程进展由宿主自动记录，无需调用通知工具。
保留用户配置中的任务目标、语言和输出格式要求。"""


class ExplorationCancelled(RuntimeError):
    pass


class ExplorationTimeout(RuntimeError):
    pass


def exploration_directory(user_data_dir, params):
    """普通聊天运行目录仅由宿主分配；旧项目执行仍沿用 profile/runs。"""
    if "temporaryDirectory" not in params:
        return Path(user_data_dir) / "runs" / str(uuid4())
    raw = params["temporaryDirectory"]
    roots = allowed_file_roots(params)
    if (
        "allowedFileRoots" not in params
        or not roots
        or not isinstance(raw, str)
        or not Path(raw).is_absolute()
    ):
        raise ValueError("temporaryDirectory 必须为宿主分配的会话内绝对目录")
    # 多根协议的首项固定为会话临时根，工程目录不能承载浏览器中间产物。
    session_root = roots[0].resolve()
    directory = Path(raw).resolve()
    if directory == session_root or not directory.is_relative_to(session_root):
        raise ValueError("浏览器临时运行目录必须位于本会话临时目录内")
    return directory


class PersistentAgent(Agent):
    async def _handle_step_error(self, error):
        # 固定版本 SDK 会把步骤异常转换为 history；上下文准备失败甚至没有历史项。
        self.execution_diagnostics.step_failed(error)
        await super()._handle_step_error(error)

    def _log_agent_event(self, max_steps, agent_run_error=None):
        # 仅观察既有停止原因，保持上游循环、清理和 telemetry 语义。
        self.execution_diagnostics.stop_reason = agent_run_error
        return super()._log_agent_event(max_steps, agent_run_error)

    def _set_file_system(self, file_system_path=None):
        # 截图与抽取产物同样归属宿主分配的运行目录，避免上游另建全局临时目录。
        if file_system_path is None:
            raise ValueError("探索 Agent 必须指定运行目录")
        self.agent_directory = Path(file_system_path)
        super()._set_file_system(file_system_path)

    async def close(self):
        # BrowserSession 的生命周期由 sidecar 管理，不能停止供 UI 共用的事件总线。
        if self.skill_service is not None:
            await self.skill_service.close()


class ObservedTools(Tools):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        for name in list(self.registry.registry.actions):
            if name not in ALLOWED_ACTIONS:
                self.exclude_action(name)

    async def act(self, action, browser_session, **kwargs):
        values = action.model_dump(exclude_unset=True)
        if len(values) != 1:
            return ActionResult(error="一次只能执行一个浏览器动作")
        name, arguments = next(iter(values.items()))
        if name not in ALLOWED_ACTIONS:
            return ActionResult(error=f"禁止动作：{name}")
        if self.owner.cancelled.is_set():
            raise asyncio.CancelledError
        self.owner.action_count += 1
        task_id = f"{self.owner.run_id}-action-{self.owner.action_count}"
        self.owner.emit(
            "action_started", taskId=task_id, toolName=name, arguments=arguments
        )
        try:
            roots = allowed_file_roots(self.owner.params)
            if name == "navigate":
                require_navigation_allowed(arguments.get("url"), roots)
            require_navigation_allowed(
                await browser_session.get_current_page_url(), roots
            )
            result = await super().act(action, browser_session, **kwargs)
            # 后退、切换标签和点击导航也必须重新确认目的地。
            require_navigation_allowed(
                await browser_session.get_current_page_url(), roots
            )
        except asyncio.CancelledError:
            self.owner.emit(
                "action_finished",
                taskId=task_id,
                toolName=name,
                success=False,
                resultPreview="已取消",
                error="已取消",
            )
            raise
        except Exception as error:
            self.owner.emit(
                "action_finished",
                taskId=task_id,
                toolName=name,
                success=False,
                resultPreview=self.owner.safe(str(error)),
                error=self.owner.safe(str(error)),
            )
            return ActionResult(error=self.owner.safe(str(error)))
        self.owner.emit(
            "action_finished",
            taskId=task_id,
            toolName=name,
            success=not bool(result.error),
            error=self.owner.safe(result.error) if result.error else None,
            resultPreview=self.owner.safe(
                result.error
                or result.extracted_content
                or result.long_term_memory
                or "完成"
            )[:2000],
        )
        return result


class Explorer:
    def __init__(self, runtime, params, llm=None):
        self.runtime = runtime
        self.params = params
        self.run_id = str(params["runId"])
        self.cancelled = asyncio.Event()
        self.task = None
        self.agent = None
        self.action_count = 0
        self.llm = llm
        self.secret = str(params.get("model", {}).get("apiKey") or "")
        register_secret(self.secret)
        self.diagnostics = RunDiagnostics(self.emit)

    def safe(self, value):
        text = str(value)
        return text.replace(self.secret, "[REDACTED]") if self.secret else text

    def emit(self, kind, **data):
        self.runtime.agent_event(self.run_id, kind, data)

    def usage(self):
        if self.agent is None:
            return {"promptTokens": 0, "completionTokens": 0, "totalTokens": 0}
        entries = self.agent.token_cost_service.usage_history
        prompt = sum(item.usage.prompt_tokens for item in entries)
        completion = sum(item.usage.completion_tokens for item in entries)
        return {
            "promptTokens": prompt,
            "completionTokens": completion,
            "totalTokens": prompt + completion,
        }

    async def progress(self, state, output, step):
        # 不输出 thinking / memory，界面只接收可观察状态及执行中的步骤数。
        self.emit("progress", step=step, url=state.url)
        self.emit("usage", tokenUsage=self.usage())

    def cancel(self):
        self.cancelled.set()
        if self.task is not None:
            self.task.cancel()

    async def execute(self):
        max_steps = int(self.params.get("maxSteps", 30))
        timeout = float(self.params.get("timeoutSecs", 600))
        if not 1 <= max_steps <= 500 or not 0 < timeout <= 3600:
            raise ValueError("maxSteps 必须为 1..500，timeoutSecs 必须为 (0,3600]")
        task = str(self.params.get("task", "")).strip()
        if not task:
            raise ValueError("探索任务不能为空")
        use_vision = self.params.get("useVision", False)
        if not isinstance(use_vision, bool):
            raise ValueError("useVision 必须为布尔值")
        directory = exploration_directory(self.runtime.user_data_dir, self.params)
        directory.mkdir(parents=True, exist_ok=True)
        downloads = directory / "downloads"
        downloads.mkdir()
        self.runtime.browser.browser_profile.downloads_path = str(downloads)
        await self.runtime.browser.cdp_client.send.Browser.setDownloadBehavior(
            params={
                "behavior": "allow",
                "downloadPath": str(downloads),
                "eventsEnabled": True,
            }
        )
        self.agent = PersistentAgent(
            task=task,
            llm=self.llm or model_from_config(self.params["model"]),
            browser_session=self.runtime.browser,
            tools=ObservedTools(self),
            extend_system_message="\n\n".join(
                [str(self.params.get("systemPrompt") or ""), ENGINE_BOUNDARY]
            ),
            max_actions_per_step=1,
            max_failures=3,
            use_judge=False,
            use_thinking=False,
            use_vision=use_vision,
            enable_signal_handler=False,
            file_system_path=str(directory),
            calculate_cost=False,
            directly_open_url=False,
            final_response_after_failure=False,
            register_new_step_callback=self.progress,
        )
        # 上游按旧模型名单强制关闭部分视觉能力；宿主以用户显式设置为准。
        self.agent.settings.use_vision = use_vision
        self.agent.execution_diagnostics = self.diagnostics
        self.task = asyncio.create_task(
            self.agent.run(
                max_steps=max_steps, on_step_start=self.diagnostics.step_started
            )
        )
        if self.cancelled.is_set():
            self.task.cancel()

        def consume_finished_task(task):
            if not task.cancelled():
                error = task.exception()
                if error is not None and self.cancelled.is_set():
                    self.runtime.log(f"已取消探索的收尾异常：{type(error).__name__}")

        self.task.add_done_callback(consume_finished_task)
        history = None
        failure = None
        try:
            # Agent.run 某些启动路径会吞掉 CancelledError；wait_for 可能因此返回
            # 一个未完成 history。独立检查 deadline，超时绝不能被误报为完成。
            done, _ = await asyncio.wait({self.task}, timeout=timeout)
            if not done:
                self.cancel()
                failure = "浏览器探索超时"
                await asyncio.wait({self.task}, timeout=2)
            else:
                history = self.task.result()
        except asyncio.CancelledError:
            self.cancelled.set()
            failure = "浏览器探索已取消"
        except Exception as error:
            failure = exception_diagnostic(error)
        finally:
            self.emit("usage", tokenUsage=self.usage())
            if self.cancelled.is_set():
                await self.runtime.close_browser()
        if self.cancelled.is_set():
            if failure == "浏览器探索超时":
                raise ExplorationTimeout(f"[timeout] {failure}")
            raise ExplorationCancelled(f"[cancelled] {failure or '浏览器探索已取消'}")
        history = history if history is not None else self.agent.history
        success, result = self.diagnostics.result(
            self.agent, history, max_steps, failure=failure
        )
        return {
            "success": success,
            "result": self.safe(result),
            "iterations": self.diagnostics.attempts,
            "tokenUsage": self.usage(),
            "urls": [url for url in history.urls() if url] if history else [],
        }
