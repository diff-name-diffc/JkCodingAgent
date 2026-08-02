//! 图编排（Graph Orchestrator）模块。
//!
//! 项目 Agent 的核心产物是执行图（DAG）：编排阶段由 OrchestratorAgent 通过
//! `submit_graph` 工具产出 `GraphDefinition`，经 `validate` 校验后落库；
//! 执行阶段由 `runner` 按拓扑分层调度 `node_exec` 的节点执行器，
//! 节点间通过共享 state 流转数据，全程通过 `graph-run-event` 全局广播进展。

pub(crate) mod commands;
pub(crate) mod input;
pub(crate) mod node_exec;
pub(crate) mod node_task;
pub(crate) mod runner;
pub(crate) mod store;
pub mod types;
pub(crate) mod validate;

pub(crate) use store::GraphStore;
