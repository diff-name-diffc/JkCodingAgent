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

/// 用户配置的工具超时默认（秒）：`AhaSettingsV2.toolTimeouts` 经
/// apply_settings_v2 解析后随 deps 下传（子智能体/程序叶子经 deps 克隆
/// 自动继承）。None = 未配置，回退策略表默认；有效值由
/// `spec::effective_timeout_secs` 统一解析（调用声明 > 本默认 > 表默认），
/// 键与白名单工具一一对应。
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolTimeoutDefaults {
    pub generate_image: Option<u64>,
    pub edit_image: Option<u64>,
    pub fetch_image: Option<u64>,
}

impl ToolTimeoutDefaults {
    /// 按工具名取用户配置默认（白名单外恒 None）。
    pub(crate) fn for_tool(&self, name: &str) -> Option<u64> {
        match name {
            "generate_image" => self.generate_image,
            "edit_image" => self.edit_image,
            "fetch_image" => self.fetch_image,
            _ => None,
        }
    }
}

impl From<&crate::agent::db::ToolTimeoutSettings> for ToolTimeoutDefaults {
    fn from(settings: &crate::agent::db::ToolTimeoutSettings) -> Self {
        Self {
            generate_image: settings.generate_image_secs.map(u64::from),
            edit_image: settings.edit_image_secs.map(u64::from),
            fetch_image: settings.fetch_image_secs.map(u64::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolTimeoutDefaults;

    /// for_tool 必须路由 spec.rs `call_timeout_range` 白名单的全部工具名：
    /// 漏路由一个名字，该工具的用户配置默认会静默失效（调用声明仍生效，
    /// 问题难以察觉）。用互异值断言「名字 → 正确字段」，白名单扩容时
    /// 本用例与 spec.rs 的同步清单（call_timeout_range doc）共同提醒补路由。
    #[test]
    fn for_tool_routes_call_timeout_whitelist_names() {
        let defaults = ToolTimeoutDefaults {
            generate_image: Some(11),
            edit_image: Some(22),
            fetch_image: Some(33),
        };
        for (name, expected) in [
            ("generate_image", Some(11)),
            ("edit_image", Some(22)),
            ("fetch_image", Some(33)),
            ("read_file", None),
            ("no_such_tool", None),
        ] {
            assert_eq!(defaults.for_tool(name), expected, "路由：{name}");
        }
    }

    /// `From<&ToolTimeoutSettings>` 的字段映射不错位（u32 → u64 逐键对应）。
    #[test]
    fn converts_settings_fields_without_mixing() {
        let settings = crate::agent::db::ToolTimeoutSettings {
            generate_image_secs: Some(30),
            edit_image_secs: Some(40),
            fetch_image_secs: Some(50),
        };
        let defaults = ToolTimeoutDefaults::from(&settings);
        assert_eq!(defaults.generate_image, Some(30));
        assert_eq!(defaults.edit_image, Some(40));
        assert_eq!(defaults.fetch_image, Some(50));
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
    /// 用户配置的工具超时默认（白名单工具的 HTTP 预算解析用）。
    pub tool_timeouts: ToolTimeoutDefaults,
    /// 命令类工具的安全审查上下文（local_zsh / ssh_exec / sync_directory /
    /// MCP 桥在执行前带完整目标环境上下文做 fail-closed 审查）。
    pub review: super::super::review::RigReviewContext,
}
