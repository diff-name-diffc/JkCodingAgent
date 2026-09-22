//! 架构设计 Agent（`RigArchitectureAgent`）：主模型即视觉模型，
//! 工具面仅 `architecture_run`（画布程序）。

mod agent;

pub use agent::{ArchitectureTurnRequest, RigArchitectureAgent};
