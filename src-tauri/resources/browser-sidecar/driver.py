"""JSONL 协议入口；stdout 只用于协议，第三方日志全部发 stderr。"""

import asyncio
import json
import os
import sys

protocol_stdout = sys.stdout
sys.stdout = sys.stderr

import bootstrap  # noqa: E402,F401
from diagnostics import exception_diagnostic  # noqa: E402
from runtime import BrowserRuntime  # noqa: E402


def write(value):
    protocol_stdout.write(
        json.dumps(bootstrap.redact_value(value), ensure_ascii=False, allow_nan=False)
        + "\n"
    )
    protocol_stdout.flush()


async def main():
    runtime = BrowserRuntime(os.environ.get("JKC_BROWSER_SESSION_ID", "browser"), write)
    jobs = set()

    async def handle(request):
        request_id = request.get("id")
        try:
            method = request["method"]
            params = request.get("params") or {}
            if not isinstance(params, dict):
                raise ValueError("params 必须是 JSON 对象")
            if method == "start":
                result = await runtime.start(params)
            elif method == "explore":
                result = await runtime.explore(params)
            elif method == "cancel":
                result = await runtime.cancel(str(params["runId"]))
            elif method == "close":
                result = await runtime.close()
            elif method == "status":
                result = runtime.status()
            elif method == "set_preview":
                result = runtime.set_preview(params)
            elif method == "screenshot":
                result = await runtime.screenshot()
            else:
                result = await runtime.manual(method, params)
            write({"id": request_id, "ok": True, "result": result})
        except Exception as error:
            params = request.get("params")
            config = params.get("model") if isinstance(params, dict) else None
            secret = str(config.get("apiKey") or "") if isinstance(config, dict) else ""
            if secret:
                bootstrap.register_secret(secret)
            message = exception_diagnostic(error)
            write(
                {
                    "id": request_id,
                    "ok": False,
                    "error": message,
                    "errorType": type(error).__name__,
                }
            )

    reader = asyncio.StreamReader(limit=2 * 1024 * 1024)
    protocol = asyncio.StreamReaderProtocol(reader)
    transport, _ = await asyncio.get_running_loop().connect_read_pipe(
        lambda: protocol, sys.stdin
    )
    try:
        while True:
            try:
                line = await reader.readline()
            except ValueError:
                # 单行超过 2 MiB 上限：丢弃该行并回执错误，不能让读循环退出
                # （超长 systemPrompt/task 不应杀掉整条浏览器会话）。
                write({"id": None, "ok": False, "error": "请求行超过 2 MiB 上限，已丢弃"})
                continue
            if not line:
                break
            try:
                request = json.loads(line)
                if not isinstance(request, dict):
                    raise ValueError("请求必须是 JSON 对象")
            except (ValueError, UnicodeDecodeError) as error:
                write({"id": None, "ok": False, "error": str(error)})
                continue
            job = asyncio.create_task(handle(request))
            jobs.add(job)
            job.add_done_callback(jobs.discard)
    finally:
        transport.close()
        for job in jobs:
            job.cancel()
        await asyncio.gather(*jobs, return_exceptions=True)
        await runtime.close()


if __name__ == "__main__":
    asyncio.run(main())
