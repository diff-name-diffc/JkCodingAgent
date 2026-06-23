"""RAG sidecar 服务入口。

启动协议（与 Rust 宿主 src-tauri/src/rag/manager.rs 约定）：
  1. 宿主 spawn 本进程时，通过环境变量注入初始知识库配置
     （详见 config.RagSettings.from_env）。
  2. 本进程选择一个空闲端口启动 uvicorn，并在 stdout 第一行
     打印一条握手消息，格式严格如下（单行 JSON，无前后空行）：
        RAG_LISTENING {"port": 54321, "pid": 12345}
     宿主的 stdout reader 解析到该行后，用 reqwest 访问
     http://127.0.0.1:<port>/* 与本进程通信。
  3. 此后所有正常日志应写到 stderr，避免污染 stdout 的协议通道。

开发模式（不经 PyInstaller）：
    cd rag && uv run python -m rag_server
    或：uv run uvicorn rag_server.main:app --port 8765
"""

from __future__ import annotations

import json
import os
import sys
from contextlib import asynccontextmanager
from typing import AsyncIterator

import uvicorn
from fastapi import FastAPI

from . import __version__
from .config import init_settings, normalize_log_level
from .routers import config_router, health_router


def _build_app() -> FastAPI:
    """构造 FastAPI 应用（供测试与 uvicorn 复用）。"""

    @asynccontextmanager
    async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
        # 启动时从环境变量初始化内存配置单例
        init_settings()
        yield

    app = FastAPI(
        title="JKCodingAgent RAG Sidecar",
        version=__version__,
        lifespan=lifespan,
    )
    app.include_router(health_router.router)
    app.include_router(config_router.router)
    return app


# uvicorn 通过模块路径 `rag_server.main:app` 引用此对象
app = _build_app()


def _emit_handshake(port: int) -> None:
    """向 stdout 打印单行握手消息。

    格式：`RAG_LISTENING <json>`，宿主按此前缀匹配解析。
    必须在 uvicorn 真正开始监听后、第一条业务日志前发出。
    """
    payload = {"port": port, "pid": os.getpid(), "version": __version__}
    sys.stdout.write(f"RAG_LISTENING {json.dumps(payload, ensure_ascii=False)}\n")
    sys.stdout.flush()


def _resolve_port() -> int:
    """解析监听端口。

    优先级：
      1. 环境变量 RAG_PORT（宿主可指定固定端口便于调试）
      2. 0 表示让 OS 分配空闲端口（生产路径，握手时回传真实端口）

    注意：uvicorn 以 port=0 启动后，实际端口需从 server.socket 取得，
    本骨架用固定的 RAG_PORT 调试路径；生产建议在 lifespan 中读取
    真实端口后再发握手（见下方注释 TODO）。
    """
    raw = os.environ.get("RAG_PORT")
    if raw and raw.isdigit():
        return int(raw)
    return 0


def main() -> None:
    """PyInstaller 产物入口；开发模式下也可直接 `python -m rag_server` 调用。"""
    # 日志统一走 stderr，避免污染 stdout 协议通道
    log_level = normalize_log_level(os.environ.get("RAG_LOG_LEVEL", "info")).lower()

    port = _resolve_port()

    # 生产路径建议：port=0 让 OS 分配，然后在 uvicorn lifespan 中读取真实端口
    # 再调用 _emit_handshake。骨架阶段支持固定端口，握手直接发。
    if port != 0:
        _emit_handshake(port)

    uvicorn.run(
        app,
        host="127.0.0.1",
        port=port,
        log_level=log_level,
        # 让 uvicorn 的访问日志/错误日志写到 stderr
        access_log=False,
    )


if __name__ == "__main__":
    main()
