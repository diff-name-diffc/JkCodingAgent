// 迁移期过渡（Phase 3 → Phase 5）：普通聊天已切到 rig 运行时（`rig_ext`），
// 旧执行路径（`llm` / `run_loop` / `agents::project|architecture` / `tools` 旧
// 基础设施 / `common` 旧助手）在 Phase 5 删除前不再被聊天路径引用，因此产生
// 大量死代码告警。迁移完成后必须删除本 allow（Phase 5 检查项）。
#![allow(dead_code)]

pub(crate) mod agents;
pub(crate) mod command_history;
pub mod commands;
pub(crate) mod common;
pub(crate) mod config;
pub(crate) mod db;
pub(crate) mod debug;
pub(crate) mod graph;
pub(crate) mod llm;
mod prompt;
pub(crate) mod rig_ext;
mod run_loop;
pub(crate) mod ssh_review;
mod state;
pub mod sub_agent;
pub mod tools;

pub use state::DispatcherState;
