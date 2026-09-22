//! 架构设计 Agent（rig 形态）：`ArchitectureAgent` 的替代实现。
//! 主模型即视觉模型，工具面仅 `architecture_run`（画布程序）。

mod agent;

pub use agent::{ArchitectureTurnRequest, RigArchitectureAgent};
