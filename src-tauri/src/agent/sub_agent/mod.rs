//! 子智能体配置与持久化。
//!
//! 运行时（循环/工具/事件）已迁移到 rig 实现：见
//! `crate::agent::rig_ext::sub_agent`。本模块只保留配置、管理器与 DB。

pub mod commands;
pub mod config;
pub mod db;
pub mod manager;

pub use manager::SubAgentManager;
