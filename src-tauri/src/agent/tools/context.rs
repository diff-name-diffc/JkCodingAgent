use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use tauri::AppHandle;
use tokio::sync::watch;

use crate::agent::db::settings::SshReviewConfig;
use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::tools::registry::ToolRegistry;
use crate::mcp::McpScope;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_id: String,
    /// 工作区根目录。路径边界输入之一：执行入口（`CapabilityBroker::invoke`）
    /// 会统一调用 `normalize_paths` 将其 canonicalize，下游所有路径比较
    /// （resolve_path / restrict_to_workspace）只与规范化后的路径进行。
    /// 注意：本字段只承担文件沙箱职责；MCP 工具可见面由 `mcp_scope` 决定。
    pub workspace: PathBuf,
    /// MCP 配置作用域：决定动态（MCP）工具的枚举与执行走哪份合并配置。
    /// 普通聊天恒为 `McpScope::Global`（所有会话共享）；项目运行（编排器、
    /// 图节点）为 `McpScope::Project(项目根)`。子智能体克隆父上下文继承。
    pub mcp_scope: McpScope,
    pub session_title: String,
    /// 当前用户任务（最新一条用户消息文本），供 SSH 安全审查等场景使用。
    pub user_task: Option<String>,
    /// SSH 命令安全审查 AI 配置；None 表示未配置，跳过审查。
    /// 内含模型 API Key，Debug 输出已脱敏。
    pub ssh_review: Option<SshReviewConfig>,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    /// 额外允许访问的路径白名单（目录前缀或精确文件；不受
    /// restrict_to_workspace 限制）。
    /// 路径边界输入之一：执行入口会统一 canonicalize，解析失败的条目会被剔除，
    /// 防止相对路径/符号链接目录绕过白名单比较。
    pub extra_allowed_dirs: Vec<PathBuf>,
    pub app_handle: Option<AppHandle>,
    /// 内含 API Key，Debug 输出已脱敏。
    pub llm_provider: Option<OpenAiCompatProvider>,
    pub vision_model: String,
    pub image_model_url: String,
    /// 敏感凭据：仅在此字段明文持有。Debug 输出已脱敏；
    /// 新增日志、错误消息或序列化输出时严禁携带该值。
    pub image_model_api_key: String,
    pub image_model: String,
    pub image_edit_model: String,
    pub sub_agent_tool_registry: Option<Arc<ToolRegistry>>,
    /// 注入不变量：本字段与下方两个关联 ID 均由执行入口按「先 clone、写入后
    /// 再传入工具执行」的模式注入（统一入口：`CapabilityBroker::invoke`；
    /// 子智能体：`sub_agent/runtime.rs`）。不要从未注入的 clone 上读取并
    /// 假定其有值，也不要在工具执行期间再次 clone 后改写字段。
    pub current_sub_agent_id: Option<String>,
    pub current_sub_agent_name: Option<String>,
    /// 当前正在执行的能力调用 ID，由 CapabilityBroker 在调用前注入。
    pub current_tool_call_id: Option<String>,
    /// prepare_input 时绑定的 ToolSpec 摘要。动态 provider 必须在实际副作用前
    /// 对照最新目录复核，防止同名 MCP 工具在准备与执行之间发生 TOCTOU 漂移。
    pub current_tool_spec_hash: Option<String>,
    /// 由 CapabilityBroker 注入的协作式取消信号。命令/长扫描类工具应主动
    /// 消费该信号并终止底层子进程或循环，不能只依赖上层 drop future。
    pub cancel_rx: Option<watch::Receiver<bool>>,
    /// 父级 call_sub_agent 的工具调用 ID，供子智能体内部事件稳定关联。
    pub sub_agent_parent_tool_call_id: Option<String>,
    /// 子智能体运行期间聚合的可持久化事件；根 Agent 上下文中为 None。
    /// 注意：当前为无容量上限的共享缓冲，容量治理需在事件写入方
    /// （sub_agent/runtime.rs 的 record_trace_event）实施。
    pub sub_agent_trace_events: Option<Arc<Mutex<Vec<Value>>>>,
}

impl ToolContext {
    /// 在执行入口统一规范化 `workspace` 与 `extra_allowed_dirs` 两个路径边界
    /// 输入（G1-23）：保证后续所有工具只与 canonical 形式的路径比较，
    /// 杜绝相对路径或符号链接绕过目录白名单。
    ///
    /// best-effort 语义（不因此中断工具执行）：
    /// - `workspace` canonicalize 失败时保留原值——虚拟/占位工作区在个别
    ///   场景下可能尚未落盘，边界校验仍由 resolve_path 按原逻辑兜底；
    /// - `extra_allowed_dirs` 中解析失败的条目直接剔除（白名单收紧是
    ///   fail-closed 方向）并记录日志。
    pub fn normalize_paths(&mut self) {
        match self.workspace.canonicalize() {
            Ok(canonical) => self.workspace = canonical,
            Err(error) => {
                eprintln!(
                    "failed to canonicalize tool workspace {}: {error}",
                    self.workspace.display()
                );
            }
        }

        let dirs = std::mem::take(&mut self.extra_allowed_dirs);
        let mut normalized = Vec::with_capacity(dirs.len());
        for dir in dirs {
            match dir.canonicalize() {
                Ok(canonical) => normalized.push(canonical),
                Err(error) => {
                    eprintln!(
                        "dropping unresolvable extra allowed path {}: {error}",
                        dir.display()
                    );
                }
            }
        }
        self.extra_allowed_dirs = normalized;
    }
}

/// 手写 Debug：敏感凭据（image_model_api_key、llm_provider / ssh_review 内含的
/// API Key）一律脱敏，避免上下文进入日志、调试快照或错误消息时泄露明文。
impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("workspace_id", &self.workspace_id)
            .field("workspace", &self.workspace)
            .field("mcp_scope", &self.mcp_scope)
            .field("session_title", &self.session_title)
            .field("user_task", &self.user_task)
            .field(
                "ssh_review",
                &self
                    .ssh_review
                    .as_ref()
                    .map(|_| "<redacted: contains api key>"),
            )
            .field("exec_timeout_secs", &self.exec_timeout_secs)
            .field("restrict_to_workspace", &self.restrict_to_workspace)
            .field("extra_allowed_dirs", &self.extra_allowed_dirs)
            .field("app_handle", &self.app_handle.as_ref().map(|_| "<present>"))
            .field(
                "llm_provider",
                &self
                    .llm_provider
                    .as_ref()
                    .map(|_| "<redacted: contains api key>"),
            )
            .field("vision_model", &self.vision_model)
            .field("image_model_url", &self.image_model_url)
            .field("image_model_api_key", &"<redacted>")
            .field("image_model", &self.image_model)
            .field("image_edit_model", &self.image_edit_model)
            .field(
                "sub_agent_tool_registry",
                &self.sub_agent_tool_registry.as_ref().map(|_| "<present>"),
            )
            .field("current_sub_agent_id", &self.current_sub_agent_id)
            .field("current_sub_agent_name", &self.current_sub_agent_name)
            .field("current_tool_call_id", &self.current_tool_call_id)
            .field("current_tool_spec_hash", &self.current_tool_spec_hash)
            .field("cancel_rx", &self.cancel_rx.as_ref().map(|_| "<present>"))
            .field(
                "sub_agent_parent_tool_call_id",
                &self.sub_agent_parent_tool_call_id,
            )
            .field(
                "sub_agent_trace_events",
                &self.sub_agent_trace_events.as_ref().map(|_| "<present>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ToolContext;

    fn context_with_dirs(
        workspace: std::path::PathBuf,
        extra: Vec<std::path::PathBuf>,
    ) -> ToolContext {
        ToolContext {
            workspace_id: "ws".to_string(),
            workspace,
            mcp_scope: crate::mcp::McpScope::Global,
            session_title: "t".to_string(),
            user_task: None,
            ssh_review: None,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            extra_allowed_dirs: extra,
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
        }
    }

    #[test]
    fn normalize_paths_canonicalizes_workspace_and_drops_unresolvable_dirs() {
        let temp = std::env::temp_dir();
        let mut context = context_with_dirs(
            temp.clone(),
            vec![
                temp.clone(),
                // 不存在的目录：白名单条目应被剔除（fail-closed）。
                std::path::PathBuf::from("/definitely/not/existing/dir/xyz"),
            ],
        );

        context.normalize_paths();

        assert_eq!(context.workspace, temp.canonicalize().unwrap());
        assert_eq!(
            context.extra_allowed_dirs,
            vec![temp.canonicalize().unwrap()]
        );
    }

    #[test]
    fn normalize_paths_keeps_missing_workspace_best_effort() {
        let missing = std::path::PathBuf::from("/definitely/not/existing/workspace/xyz");
        let mut context = context_with_dirs(missing.clone(), Vec::new());

        context.normalize_paths();

        // 工作区不存在时保留原值，不中断工具执行。
        assert_eq!(context.workspace, missing);
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let mut context = context_with_dirs(std::env::temp_dir(), Vec::new());
        context.image_model_api_key = "sk-super-secret".to_string();

        let rendered = format!("{context:?}");

        assert!(!rendered.contains("sk-super-secret"));
        assert!(rendered.contains("<redacted>"));
    }
}
