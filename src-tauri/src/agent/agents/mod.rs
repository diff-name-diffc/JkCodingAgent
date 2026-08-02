//! 顶层 Agent 实现集合。
//!
//! `run_loop` 只定义统一运行骨架；这里放真正的 Agent 形态：
//! - `project::OrchestratorAgent`：面向项目工作区，只读探索 + 执行图编排。
//! - `plain_chat::PlainChatAgent`：面向普通聊天工作区，直接工具循环。

pub(crate) mod plain_chat;
pub(crate) mod project;

pub(crate) use plain_chat::PlainChatAgent;
pub(crate) use project::OrchestratorAgent;
