//! rig 工具层（Phase 2）：全部内建工具以 rig `PortableDynamicTool` 形态产出，
//! 由 `loop::surface::RigToolSurface` 收纳进运行时循环。
//!
//! 分组与入口（每组一个文件/目录，组内可再拆子模块）：
//! - `fs`：read_file / list_dir / glob / grep（只读数据面，编排器也用）；
//! - `exec`：local_zsh / ssh_* / ssh_memo_* / sync_directory（命令执行面）；
//! - `media`：generate_image / edit_image / analyze_image / fetch_image / browser_*；
//! - `program`：run_tool_program（工具程序 DSL 执行器，数据面工具由调用方注入）；
//! - `mcp`：MCP 动态工具桥（`mcp__<server>__<tool>`）。
//!
//! 协议类工具（submit_graph / graph_plan_report / architecture_run /
//! notify_user_progress / 子智能体工具）不在本层——它们随 Phase 3 各 agent
//! 工厂就近定义（与各自的 runtime 拦截逻辑同源）。
//!
//! 约定：
//! - 工具一律产出 `PortableDynamicTool`（typed `PortableTool` 编写后经
//!   擦除包装亦可），业务逻辑从 `agent/tools/builtin/` 对应文件迁移过来
//!   （旧文件 Phase 5 删除，本层不得 import 旧 builtin 模块）；
//! - 路径参数一律经 `common::resolve_path`；错误消息以「错误：」开头；
//! - 压缩阈值/内联上限常量统一从 `super::tool_result` 取用，schema 文案
//!   用 `common::with_compression_parameters` 注入，禁止两处口径漂移。

pub(crate) mod common;
pub(crate) mod deps;
pub(crate) mod exec;
pub(crate) mod fs;
pub(crate) mod mcp;
pub(crate) mod media;
pub(crate) mod program;
pub(crate) mod run_record;

pub(crate) use deps::RigToolDeps;
