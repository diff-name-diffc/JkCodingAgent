# RAG Sidecar（rag-server）

JKCodingAgent 的 RAG 服务子进程，由 Tauri 宿主以 sidecar 方式启动。

> 当前阶段仅提供**可运行的结构骨架**，不包含真实的 embedding / ingestion /
> retrieval 实现。目的是先把宿主 ↔ sidecar 的启动、握手、配置流转链路打通。

## 架构定位

```
┌──────────────────────────────────────────────────────────────┐
│ Tauri 宿主（Rust）                                            │
│                                                              │
│  src-tauri/src/rag/                                          │
│   ├── config.rs   知识库配置权威存储 ~/.jkcodingagent/rag/   │
│   ├── manager.rs  sidecar 启停 + 端口握手                    │
│   └── transport.rs  HTTP 调用 sidecar                        │
│                │                                             │
│                │ spawn（env 注入配置）+ HTTP                 │
│                ▼                                             │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ rag-server（本工程，PyInstaller 单二进制）             │  │
│  │  FastAPI on 127.0.0.1:<动态端口>                       │  │
│  │   GET  /health                                         │  │
│  │   GET  /config          （脱敏查看）                   │  │
│  │   POST /config/reload   （宿主推送新配置）             │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
                          │
                          ▼
                   外部 Qdrant 实例
```

### 启动握手协议

1. 宿主 spawn 本进程，通过环境变量注入初始知识库配置
   （`RAG_QDRANT_URL` / `RAG_EMBEDDING_*` 等，详见 `config.py`）。
2. 本进程选定端口后，在 **stdout 第一行** 打印：
   ```
   RAG_LISTENING {"port":54321,"pid":12345,"version":"0.1.0"}
   ```
3. 宿主解析到该行后，通过 `http://127.0.0.1:<port>` 访问本进程。
4. 本进程业务日志一律写 **stderr**，避免污染 stdout 协议通道。

## 开发

```bash
cd rag
uv sync                       # 安装依赖（含 dev）
uv run python -m rag_server   # 直接启动（开发模式，不打包）
# 或指定固定端口：
RAG_PORT=8765 uv run python -m rag_server
```

健康检查：
```bash
curl http://127.0.0.1:8765/health
```

代码检查：
```bash
uv run ruff check src tests
```

## 打包为 Tauri sidecar

Tauri 2.0 的 sidecar 通过 `bundle.externalBin` 声明，运行时自动按
target-triple 追加后缀查找二进制，例如：

| 平台 | 期望文件名 |
|------|-----------|
| macOS Apple Silicon | `rag-server-aarch64-apple-darwin` |
| macOS Intel | `rag-server-x86_64-apple-darwin` |
| Windows x64 | `rag-server-x86_64-pc-windows-msvc.exe` |
| Linux x64 | `rag-server-x86_64-unknown-linux-gnu` |

一键打包（macOS/Linux）：
```bash
bash rag/scripts/build_sidecar.sh
```
该脚本会：
1. `uv sync --no-dev`
2. PyInstaller `--onefile` 产出 `dist/rag-server`
3. 探测当前平台 triple
4. 复制并重命名为 `src-tauri/binaries/rag-server-<triple>`

Windows：
```powershell
powershell -ExecutionPolicy Bypass -File rag\scripts\build_sidecar.ps1
```

产物会被 `src-tauri/.gitignore` 忽略（不入库），仅本地/CI 生成。

## 目录结构

```
rag/
├── pyproject.toml             # uv 项目 + 依赖 + binary_name 元数据
├── .python-version            # 3.12
├── rag_server.spec            # PyInstaller 打包配置（onefile）
├── src/rag_server/
│   ├── __init__.py
│   ├── __main__.py            # `python -m rag_server` 入口
│   ├── main.py                # FastAPI app + uvicorn 启动 + 端口握手
│   ├── config.py              # RagSettings：env 注入 + reload 内存单例
│   ├── routers/
│   │   ├── health.py          # GET /health
│   │   └── config.py          # GET /config、POST /config/reload
│   └── core/
│       ├── qdrant.py          # QdrantClient 占位工厂
│       └── embedding.py       # OpenAI 兼容 embedding 占位
├── scripts/
│   ├── build_sidecar.sh       # macOS/Linux 打包脚本
│   └── build_sidecar.ps1      # Windows 打包脚本
└── tests/
```

## 后续 TODO（不在本次骨架范围）

- [ ] 接入真实 embedding（OpenAI 兼容 API，复用宿主 LLM 配置）
- [ ] 接入真实 Qdrant 客户端，实现 collection 自动建库
- [ ] 文档切片、索引、检索接口（`/ingest`、`/query`）
- [ ] 配置 reload 后触发 Qdrant/Embedding 客户端连接池重建
- [ ] 前端知识库配置 UI
