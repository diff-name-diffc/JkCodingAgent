//! RAG 模块：通过 Tauri 2.0 sidecar 机制托管一个 Python（FastAPI）子进程，
//! 用于后续承载 RAG（检索增强生成）相关服务。
//!
//! 当前为骨架阶段，仅打通 sidecar 启停 + 配置流转链路，
//! 不包含真实的 embedding / ingestion / retrieval 实现。
//!
//! 子模块：
//! - `config`：知识库配置的权威存储（~/.jkcodingagent/rag/config.json）
//! - `manager`：sidecar 进程启停 + 端口握手
//! - `logs`：sidecar stdout/stderr 的内存滚动日志
//! - `transport`：对 sidecar 的 HTTP 调用
//! - `commands`：暴露给前端的 Tauri 命令

pub mod commands;
pub mod config;
pub mod logs;
pub mod manager;
pub mod transport;

pub use config::RagConfigStore;
pub use logs::RagLogStore;
pub use manager::RagManager;
