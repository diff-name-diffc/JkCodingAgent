//! 顶层 Agent 实现集合。
//!
//! `run_loop` 只定义统一运行骨架；这里放尚未迁移的旧 Agent 形态：
//! - `architecture::ArchitectureAgent`：面向架构设计画布，单工具视觉循环
//!   （T3.3 迁移）。
//!
//! 已迁移到 rig 实现的 Agent：`crate::agent::rig_ext::agents::plain_chat`
//! （普通聊天）与 `...::project`（项目编排器）。

pub(crate) mod architecture;

pub(crate) use architecture::ArchitectureAgent;
