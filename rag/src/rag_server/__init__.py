"""RAG sidecar 服务包。

被 Tauri 宿主（src-tauri/src/rag/manager.rs）作为子进程启动：
  - 通过环境变量接收知识库配置（Qdrant 连接、embedding 模型等）
  - 通过 stdout 首行握手告知宿主监听端口
  - 通过 HTTP 接口（/health、/config/reload 等）与宿主通信

骨架阶段不实现真实的 embedding / ingestion / retrieval 逻辑，仅提供
可运行的结构与占位接口，便于 Tauri 侧先行打通 sidecar 启停链路。
"""

__version__ = "0.1.0"
