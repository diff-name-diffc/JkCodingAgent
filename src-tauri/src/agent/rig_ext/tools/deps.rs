//! rig 工具的构造期依赖集合。
//!
//! 对齐旧运行时 `ToolContext`（已随迁移删除）的**构造期**子集：
//! 旧上下文里逐次调用注入的字段（current_tool_call_id / spec hash /
//! 子智能体 trace 缓冲等）不进本结构——它们由 runtime 执行策略
//! （`loop::surface::ToolExecutionPolicy`）在每次调用时承担。
//! 审查门禁输入（user_task / executor_task / review_conversation /
//! ssh_review 配置）同理，属策略层而非工具层。

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::AppHandle;
use tokio::sync::watch;

use crate::agent::db::DispatcherDb;
use crate::agent::sub_agent::SubAgentManager;
use crate::mcp::{McpRegistry, McpScope};
use crate::ssh_tool::SshSessionManager;

use super::super::model::PurposeModelSpec;

/// 图像生成/编辑模型的直连配置（这两个工具走独立 HTTP 图像 API，
/// 不经 LLM 槽位）。敏感凭据：`api_key` 仅在此明文持有，Debug 已脱敏。
#[derive(Clone)]
pub(crate) struct ImageToolConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub edit_model: String,
}

impl std::fmt::Debug for ImageToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageToolConfig")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("edit_model", &self.edit_model)
            .finish()
    }
}

/// 逐次调用注入槽：当前工具调用 id。
///
/// 等价于旧运行时 `current_tool_call_id` 的角色——需要与父级调用关联的
/// 工具（`call_sub_agent` 的子智能体事件/轨迹关联）在构造期拿不到调用 id，
/// 由执行策略在每次调用前写入、收尾时清除。主 Agent 循环逐次执行工具，
/// 因此不存在并发写入竞态；并行只读批中的工具不得依赖本槽位。
#[derive(Clone, Default)]
pub(crate) struct ToolCallSlot(Arc<Mutex<Option<String>>>);

impl ToolCallSlot {
    pub(crate) fn set(&self, tool_call_id: impl Into<String>) {
        *self.0.lock() = Some(tool_call_id.into());
    }

    pub(crate) fn get(&self) -> Option<String> {
        self.0.lock().clone()
    }

    /// 仅当槽位仍是 `tool_call_id` 时清除，避免误清后续调用写入的值。
    pub(crate) fn clear_if(&self, tool_call_id: &str) {
        let mut slot = self.0.lock();
        if slot.as_deref() == Some(tool_call_id) {
            *slot = None;
        }
    }
}

#[derive(Clone)]
pub(crate) struct RigToolDeps {
    pub workspace_id: String,
    /// 工作区根目录（构造方须已完成 canonicalize 规范化，语义对齐旧
    /// `ToolContext::normalize_paths`；plain chat 的虚拟工作区保留原值）。
    pub workspace: PathBuf,
    pub mcp_scope: McpScope,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    /// 额外允许访问的路径白名单（构造方已完成 canonicalize）。
    pub extra_allowed_dirs: Vec<PathBuf>,
    pub app_handle: Option<AppHandle>,
    pub db: DispatcherDb,
    pub ssh_manager: SshSessionManager,
    pub mcp_registry: McpRegistry,
    pub sub_agent_manager: Option<Arc<SubAgentManager>>,
    /// run 级协作取消信号：长命令/扫描类工具应主动消费并终止底层
    /// 子进程或循环（语义同旧运行时的 `cancel_rx`）。
    pub cancel_rx: Option<watch::Receiver<bool>>,
    /// 视觉用途槽位规格（analyze_image 用）；None = 未配置，
    /// 工具须返回明确的「错误：视觉模型未配置…」可恢复错误。
    pub vision_spec: Option<PurposeModelSpec>,
    pub image: ImageToolConfig,
    /// 命令类工具的安全审查上下文（local_zsh / ssh_exec / sync_directory /
    /// MCP 桥在执行前带完整目标环境上下文做 fail-closed 审查）。
    pub review: super::super::review::RigReviewContext,
    /// 当前工具调用 id 注入槽（子智能体工具关联父调用用，见 `ToolCallSlot`）。
    pub tool_call_id: ToolCallSlot,
}
