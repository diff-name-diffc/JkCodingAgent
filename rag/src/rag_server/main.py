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
  4. 生命周期脐带：宿主以管道 stdin spawn 本进程并只持有写端。
     宿主无论正常退出还是被 kill -9 / 崩溃，管道都会关闭，
     本进程的监视线程（见 _start_host_lifecycle_watch）读到 EOF
     后自行退出——sidecar 绝不比宿主长寿。

开发模式（不经 PyInstaller）：
    cd rag && uv run python -m rag_server
    或：uv run uvicorn rag_server.main:app --port 8765
"""

from __future__ import annotations

import json
import os
import socket
import stat
import sys
import threading
import time
from contextlib import asynccontextmanager
from typing import AsyncIterator

import uvicorn
from fastapi import FastAPI

from . import __version__
from .config import init_settings, normalize_log_level
from .routers import config_router, health_router, ingest_router, tests_router


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
    app.include_router(ingest_router.router)
    app.include_router(tests_router.router)
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


def _reserve_free_port() -> int:
    """向 OS 申请一个本机空闲端口，用于 sidecar 动态端口启动。"""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _stdin_is_host_pipe() -> bool:
    """stdin 是否为宿主接入的管道。

    只有宿主 spawn 时才会以管道连接 stdin；手工在终端运行
    （stdin 是 tty）或 `uv run uvicorn ...` 等开发方式下不启用
    生命周期监视，避免误杀手工调试的进程。
    """
    try:
        return stat.S_ISFIFO(os.fstat(sys.stdin.fileno()).st_mode)
    except (AttributeError, OSError, ValueError):
        return False


def _start_host_lifecycle_watch(server: uvicorn.Server) -> None:
    """把本进程的生命周期绑定到宿主进程（「stdin 脐带」）。

    宿主（src-tauri/src/rag/manager.rs）以 piped stdin spawn 本进程，
    只持有写端、从不写入。宿主一旦消失——无论是正常退出，还是被
    kill -9、崩溃、系统强杀——内核会关闭它的全部 fd，stdin 管道
    读端收到 EOF；监视线程随即请求本进程退出。这兜住了宿主侧任何
    退出回调都无法触发的强杀场景，保证 sidecar 绝不比宿主长寿。

    PyInstaller onefile 下 Python 进程继承 bootloader 的 stdio，
    bootloader 又继承宿主的管道，因此 EOF 可以穿透两层传到这里。
    """
    if not _stdin_is_host_pipe():
        return

    fd = sys.stdin.fileno()

    def _watch() -> None:
        try:
            while True:
                # 阻塞读：宿主存活期间管道保持打开、无数据可读；
                # 读到空字节串 = EOF = 宿主已消失。
                if not os.read(fd, 4096):
                    break
        except OSError as exc:
            # 管道异常一律按宿主已消失处理。写 stderr 便于区分「宿主正常消失
            # （EOF，静默）」与「管道故障」，不污染 stdout 协议通道。
            print(
                f"[host-lifecycle-watch] stdin 管道读异常（{exc!r}），按宿主已消失退出",
                file=sys.stderr,
                flush=True,
            )
        # 请求 uvicorn 优雅收尾：should_exit 是其跨平台的协作式停机开关
        # （等价于 Unix 上收到 SIGTERM），主循环下一 tick 即退出并执行
        # lifespan shutdown 收尾。不用 os.kill(os.getpid(), SIGTERM)：
        # Windows 上 CPython 收到 SIGTERM 直接调用 TerminateProcess 硬杀
        # 进程，不会执行任何 Python 信号处理器，优雅关停根本不会发生。
        # 宽限期后仍未退出则强杀，避免优雅 shutdown 挂起导致 sidecar
        # 僵而不死。
        server.should_exit = True
        time.sleep(10)
        os._exit(1)

    threading.Thread(
        target=_watch, name="host-lifecycle-watch", daemon=True
    ).start()


def main() -> None:
    """PyInstaller 产物入口；开发模式下也可直接 `python -m rag_server` 调用。"""
    # 日志统一走 stderr，避免污染 stdout 协议通道
    log_level = normalize_log_level(os.environ.get("RAG_LOG_LEVEL", "info")).lower()

    port = _resolve_port()
    if port == 0:
        port = _reserve_free_port()

    # 先构造 uvicorn Server（仅纯配置构造，无 I/O），再把它的引用交给
    # 生命周期监视线程——监视必须在任何可能阻塞的启动步骤之前就位，
    # 保证启动流程任何阶段宿主消失都能自杀。
    server = uvicorn.Server(
        uvicorn.Config(
            app,
            host="127.0.0.1",
            port=port,
            log_level=log_level,
            # 让 uvicorn 的访问日志/错误日志写到 stderr
            access_log=False,
        )
    )
    _start_host_lifecycle_watch(server)

    _emit_handshake(port)

    server.run()


if __name__ == "__main__":
    main()
