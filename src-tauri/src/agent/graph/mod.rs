//! 图编排（Graph Orchestrator）模块。
//!
//! 项目 Agent 的核心产物是执行图（DAG）：编排阶段由 OrchestratorAgent 通过
//! `submit_graph` 工具产出 `GraphDefinition`（v3），经 `validate` 结构+语义校验后落库；
//! 执行阶段由 `runner` 驱动 `scheduler` 的 ready-queue 状态机调度 `node_exec`
//! 节点执行器（依赖驱动、失败重试一次、断点续跑、高危写检查点），节点间通过
//! 共享 state 流转数据；收尾由 `verifier` 产出验收结论、`receipt` 把执行回执
//! 写回会话消息，完成「规划 → 执行 → 验证 → 反思」闭环。
//! 全程通过 `graph-run-event` 全局广播进展。

pub(crate) mod commands;
pub(crate) mod harness;
pub(crate) mod input;
pub(crate) mod node_exec;
pub(crate) mod node_task;
pub(crate) mod pi_rpc;
pub(crate) mod receipt;
pub(crate) mod runner;
pub(crate) mod scheduler;
pub(crate) mod store;
pub mod types;
pub(crate) mod validate;
pub(crate) mod verifier;

pub(crate) use store::GraphStore;
