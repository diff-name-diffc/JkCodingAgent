use async_trait::async_trait;

use super::context::ToolContext;
use super::result::{ToolInput, ToolResult};

/// 工具提供方统一抽象（内置注册表 ToolRegistry / 其它扩展实现）。
///
/// # 实现契约（fail-closed）
///
/// 1. **路径校验**：`execute` 内涉及的任何文件/目录路径参数，实现方必须经过
///    `ToolContext` 的工作区边界校验（`resolve_path` / `restrict_to_workspace`，
///    见 `tools/builtin/common.rs`），不得直接信任调用方传入的路径；
///    工具定义枚举时传入的 `workspace` 参数仅用于筛选该工作区可见的工具，
///    不构成对任何路径的授权。
/// 2. **阻塞隔离**：`execute` 运行在 Tokio 异步线程上，实现方必须把文件 I/O、
///    子进程、网络、Git 等重型/阻塞操作放入 `tokio::task::spawn_blocking`，
///    严禁在 async 体内直接阻塞（会拖垮 Tauri 运行时）；持锁期间禁止 I/O。
/// 3. **None 语义**：`execute` 返回 `None` 仅表示「该 provider 不处理这个工具名」，
///    **不是**静默失败；命中处理时必须返回 `Some`。
/// 4. **错误格式**：返回给 LLM 的错误消息一律以「错误：」开头——
///    `result.rs` 的 `ToolResult::from_text` 依赖该前缀区分
///    recoverable / fatal / success（另见 `TOOL_ERROR_PREFIX`）。
#[async_trait]
pub trait ToolProvider: Send + Sync {
    async fn execute(
        &self,
        name: &str,
        input: ToolInput,
        context: &ToolContext,
    ) -> Option<ToolResult>;
}
