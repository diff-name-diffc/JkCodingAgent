"""提取执行失败证据，不序列化模型输出、思考或完整浏览器历史。"""

import re

from browser_use.llm.exceptions import ModelProviderError
from pydantic import ValidationError

from bootstrap import redact


def safe_diagnostic(value, limit=2000):
    text = redact(str(value)).strip()
    text = re.sub(
        r"<(think|thinking|reasoning)>.*?(?:</\1>|$)",
        "[省略模型内部内容]",
        text,
        flags=re.IGNORECASE | re.DOTALL,
    )
    text = re.sub(
        r"""(["'](?:thinking|reasoning(?:_content)?|memory|evaluation_previous_goal|next_goal)["']\s*:\s*)(["'])(?:\\.|(?!\2).)*\2""",
        r'\1"[省略模型内部内容]"',
        text,
        flags=re.IGNORECASE | re.DOTALL,
    )
    # 旧版 SDK 的格式化 ValidationError 可能把原始模型输入拼入错误文本。
    text = re.sub(
        r"input_value=.*?(?=, input_type=|$)",
        "input_value=[省略]",
        text,
        flags=re.DOTALL,
    )
    return text if len(text) <= limit else text[:limit] + "…"


def exception_diagnostic(error):
    chain = []
    seen = set()
    previous_details = None
    model_validation_id = None
    while error is not None and id(error) not in seen and len(seen) < 3:
        seen.add(id(error))
        cause = error.__cause__ or (
            None if error.__suppress_context__ else error.__context__
        )
        # SDK 将本地解析失败包装为默认 502；它不是服务端的 HTTP 响应。
        if isinstance(error, ModelProviderError) and isinstance(cause, ValidationError):
            model_validation_id = id(cause)
            error = cause
            continue
        if isinstance(error, ValidationError):
            errors = error.errors(include_input=False, include_url=False)[:5]
            details = "; ".join(
                f"{'.'.join(map(str, item['loc'])) or '<root>'}: {item['msg']} ({item['type']})"
                for item in errors
            )
            name = f"ValidationError（{error.title}）"
            if id(error) == model_validation_id:
                phase = (
                    "JSON 解析"
                    if any(item["type"] == "json_invalid" for item in errors)
                    else "校验"
                )
                name = f"模型结构化输出{phase}失败（{error.title}，ValidationError）"
        else:
            # SDK 包装的 provider 异常常与底层 HTTP 异常拥有同一份正文。
            source = cause if cause is not None and str(cause) == str(error) else error
            body = getattr(source, "body", None)
            payload = body.get("error", body) if isinstance(body, dict) else None
            message = payload.get("message") if isinstance(payload, dict) else None
            details = safe_diagnostic(message if isinstance(message, str) else error)
            name = type(error).__name__
        status = getattr(error, "status_code", None)
        if isinstance(error, ModelProviderError):
            # SDK 也用 400/502 标记截断或本地异常，HTTP 状态必须有响应证据。
            response = getattr(cause, "response", None)
            status = getattr(response, "status_code", None)
        if type(status) is int and 100 <= status <= 599:
            name += f" (HTTP {status})"
        if details != previous_details:
            chain.append(f"{name}: {details}" if details else name)
        previous_details = details
        error = cause
    return safe_diagnostic("；原因：".join(chain), limit=4000)


class RunDiagnostics:
    def __init__(self, emit):
        self.emit = emit
        self.attempts = 0
        self.last_error = None
        self.last_error_step = 0
        self.stop_reason = None

    async def step_started(self, agent):
        self.attempts += 1
        self.emit(
            "progress",
            step=self.attempts,
            message=f"正在执行浏览器步骤 {self.attempts}",
        )

    def step_failed(self, error):
        message = exception_diagnostic(error)
        self.last_error = message
        self.last_error_step = self.attempts
        self.emit(
            "progress",
            step=self.attempts,
            message=f"浏览器步骤 {self.attempts} 失败：{message}",
        )

    def result(self, agent, history, max_steps, failure=None):
        history = history if history is not None else agent.history
        success = bool(not failure and history.is_done() and history.is_successful())
        final = history.final_result()
        if success:
            return True, final or "浏览器探索已完成"
        # ActionResult.error 不一定经过异常钩子；上下文准备失败又可能没有 history。
        values = [error for error in history.errors() if error]
        values.extend(
            item.error for item in agent.state.last_result or [] if item.error
        )
        values = [error for error in values if error != self.stop_reason]
        # 只保留最近具体错误，避免 request_id 变化把同一次失败重复展示三遍。
        detail = None
        if self.last_error and self.last_error_step == self.attempts:
            detail = self.last_error
        elif values:
            detail = safe_diagnostic(values[-1])
        elif self.last_error:
            detail = self.last_error
        if failure:
            reason = (
                "浏览器探索启动失败" if self.attempts == 0 else "浏览器探索执行失败"
            )
            detail = failure
        elif history.is_done():
            reason = "浏览器探索未完成"
        elif agent.state.stopped:
            reason = "浏览器探索已停止"
        elif agent.state.consecutive_failures >= agent.settings.max_failures:
            reason = f"浏览器探索因连续 {agent.state.consecutive_failures} 次失败而停止"
        elif self.stop_reason == "Failed to complete task in maximum steps":
            reason = f"浏览器探索达到步数限制（{max_steps} 步）"
        elif not self.attempts:
            reason = "浏览器探索启动后未产生执行步骤"
        else:
            reason = "浏览器探索结束时未收到明确的完成结果"
        parts = [reason]
        if detail:
            parts.append(detail)
        if final:
            parts.append(f"执行结果：{safe_diagnostic(final, limit=4000)}")
        return False, "；".join(parts)
