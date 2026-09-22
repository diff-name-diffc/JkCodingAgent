//! 顶层 Agent 实现集合。
//!
//! `run_loop` 只定义统一运行骨架；这里放尚未迁移的旧 Agent 形态：
//! - `project::OrchestratorAgent`：面向项目工作区，只读探索 + 执行图编排
//!   （T3.2 迁移）。
//! - `architecture::ArchitectureAgent`：面向架构设计画布，单工具视觉循环
//!   （T3.3 迁移）。
//!
//! 普通聊天已迁移到 rig 实现：`crate::agent::rig_ext::agents::plain_chat`。

pub(crate) mod architecture;
pub(crate) mod project;

pub(crate) use architecture::ArchitectureAgent;
pub(crate) use project::OrchestratorAgent;
