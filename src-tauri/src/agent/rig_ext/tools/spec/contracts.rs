use super::ToolProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具结果语义压缩的通用触发阈值：`compress=true` 且原始结果超过该字符数
/// 才调用摘要模型，不超过则直接返回原文（小结果不付额外 LLM 往返）。
/// 该值同时是 schema 文案向模型声明的阈值（`with_compression_parameters`），
/// 两处必须保持一致。
pub const DEFAULT_FORCE_COMPRESS_AFTER_CHARS: usize = 5_000;

/// 命令执行类工具（exec / local_zsh / ssh_exec）的压缩触发阈值。
/// 命令输出在 8000 字符内联预算内主模型可直接精读；8k 到该阈值之间走确定性
/// 截断兜底（完整原文保留在工具产物中），只有超过该阈值且模型显式声明压缩时，
/// 才值得多付一次串行摘要 LLM 往返。
pub const COMMAND_FORCE_COMPRESS_AFTER_CHARS: usize = 12_000;

/// 派发后的默认共享直返窗口（毫秒）：窗口内结算的调用随本批内联交付，
/// 未结算的转后台（accepted 回执 + 迟到 runtime 交付）。
pub const DEFAULT_DISPATCH_WINDOW_MS: u64 = 200;

/// 只读快工具（PARALLEL_READONLY 预设）的直返窗口（毫秒）：文件/搜索类
/// 调用通常亚秒级完成，放宽窗口让扇出批次整体内联交付，避免被后台化后
/// 模型被迫空转等待轮次。取值不超 1 秒：再慢的只读调用走后台 + 参数化
/// 等待的路径同样廉价，不值得拉长每批派发的感知延迟。
pub const READONLY_DISPATCH_WINDOW_MS: u64 = 1_000;

/// 编译期保证只读窗口是对默认窗口的「放宽」：这是 PARALLEL_READONLY 预设
/// 与整表一致性测试成立的设计前提，调参若出现倒置将静默破坏扇出批次
/// 整体内联交付的意图（运行期测试只校验行值 ↔ 常量映射，不约束相对次序）。
const _: () = assert!(READONLY_DISPATCH_WINDOW_MS >= DEFAULT_DISPATCH_WINDOW_MS);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Filesystem,
    Search,
    Shell,
    Browser,
    Image,
    Ssh,
    Mcp,
    SubAgent,
    Other,
}

impl ToolCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Search => "search",
            Self::Shell => "shell",
            Self::Browser => "browser",
            Self::Image => "image",
            Self::Ssh => "ssh",
            Self::Mcp => "mcp",
            Self::SubAgent => "sub_agent",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolSafety {
    Safe,
    ReviewRequired,
    Dangerous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccess {
    pub readonly: bool,
    pub workspace_bound: bool,
    pub requires_network: bool,
    pub mutates_filesystem: bool,
    pub mutates_external_state: bool,
}

impl ToolAccess {
    /// 只读且限定工作区内（read_file / list_dir / glob / grep）。
    pub const READONLY_WORKSPACE: Self = Self {
        readonly: true,
        workspace_bound: true,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };

    /// 外部效应：访问网络、可变更外部状态（browser 交互 / image / ssh 等）。
    pub const EXTERNAL_EFFECTS: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: true,
        mutates_filesystem: false,
        mutates_external_state: true,
    };

    /// 完整外部效应：可改文件系统、可访问网络、可变更外部状态
    /// （exec / local_zsh 等命令执行类工具，能力边界按最坏情况声明，fail-closed）。
    pub const FULL_EFFECTS: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: true,
        mutates_filesystem: true,
        mutates_external_state: true,
    };

    /// 只读但不限工作区（list_sub_agents 等枚举类查询）。
    pub const READONLY_UNBOUND: Self = Self {
        readonly: true,
        workspace_bound: false,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };

    /// 非只读但不直接落盘/变更外部状态：效果由对应子系统托管
    /// （message 最终消息、submit_workflow / workflow_plan_report 协议壳、
    /// notify_user_progress / call_sub_agent 子智能体调度）。
    pub const SUBSYSTEM_MANAGED: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionPolicy {
    pub parallelizable: bool,
    /// 统一超时（秒）。语义约定：**0 表示不设统一超时限制**——
    /// ToolRuntime 会跳过统一超时包裹（与 unified_timeout=false 等效），
    /// 由工具自管生命周期；策略表中的已知工具必须 > 0。
    pub timeout_secs: u64,
    /// 工具是否消费 run 级取消信号。自管工具的兜底上限到点时据此判断是否值得发取消
    /// （见 `loop/app_policy.rs` 的 ceiling 分支；统一超时工具恒 true）。
    pub cancellable: bool,
    #[serde(default = "default_unified_timeout")]
    pub unified_timeout: bool,
    /// 自管超时工具（`unified_timeout=false`）的兜底上限（秒）：工具自身预算之外的
    /// **最后防线**，只在工具终止路径卡死时才触发，绝不用作单次超时。
    /// 取值 = 该工具最坏合法预算（见 `self_managed_settle_ceiling_secs`）；统一超时工具恒 0。
    #[serde(default)]
    pub settle_ceiling_secs: u64,
}

impl ToolExecutionPolicy {
    pub fn sequential(timeout_secs: u64) -> Self {
        Self {
            parallelizable: false,
            timeout_secs,
            cancellable: true,
            unified_timeout: true,
            settle_ceiling_secs: 0,
        }
    }

    pub fn parallel_readonly(timeout_secs: u64) -> Self {
        Self {
            parallelizable: true,
            timeout_secs,
            cancellable: true,
            unified_timeout: true,
            settle_ceiling_secs: 0,
        }
    }

    pub fn tool_managed_timeout(timeout_secs: u64, settle_ceiling_secs: u64) -> Self {
        Self {
            parallelizable: false,
            timeout_secs,
            cancellable: true,
            unified_timeout: false,
            settle_ceiling_secs,
        }
    }
}

fn default_unified_timeout() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPolicy {
    pub default_compress: bool,
    pub force_compress_after_chars: usize,
    pub persist_raw_artifact: bool,
}

impl ToolResultPolicy {
    pub fn new(default_compress: bool) -> Self {
        Self {
            default_compress,
            force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            persist_raw_artifact: true,
        }
    }

    /// 覆盖压缩触发阈值（命令执行类工具使用更高的 `COMMAND_FORCE_COMPRESS_AFTER_CHARS`）。
    pub fn with_force_compress_after_chars(mut self, chars: usize) -> Self {
        self.force_compress_after_chars = chars;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub provider: String,
    pub category: ToolCategory,
    pub access: ToolAccess,
    pub safety: ToolSafety,
    /// 该工具在内部自管安全审查（携带完整目标环境上下文做 fail-closed 判定）。
    /// CapabilityBroker 对这类工具不再做通用 JSON 参数审查——否则同一调用会
    /// 出现两套审查标准互相覆盖（例如 per-server 审查开关会被 broker 的通用
    /// 结论短路）。目前为 local_zsh / ssh_exec / sync_directory / SSH 日志远程工具（策略表）与
    /// 全部 MCP 动态工具（ToolSpec::mcp）。
    pub review_self_managed: bool,
    pub execution: ToolExecutionPolicy,
    pub result_policy: ToolResultPolicy,
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, parameters: Value) -> Self {
        let profile = ToolProfile::from_name(name);
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            provider: "builtin".to_string(),
            category: profile.category,
            access: profile.access,
            safety: profile.safety,
            review_self_managed: profile.review_self_managed,
            execution: profile.execution,
            result_policy: profile.result_policy,
        }
    }

    pub fn mcp(name: String, description: String, parameters: Value) -> Self {
        Self {
            name,
            description,
            parameters,
            provider: "mcp".to_string(),
            category: ToolCategory::Mcp,
            access: ToolAccess {
                readonly: false,
                workspace_bound: false,
                requires_network: true,
                mutates_filesystem: false,
                mutates_external_state: true,
            },
            safety: ToolSafety::ReviewRequired,
            // MCP 桥接层（tools/mcp.rs）在执行前带工具名与完整参数做
            // fail-closed 审查，broker 不再重复做通用 JSON 审查。
            review_self_managed: true,
            // 策略层 deadline 经 effective_timeout_secs 重推导（MCP 工具不在
            // 策略表内 → 兜底 DEFAULT_UNKNOWN_TIMEOUT_SECS）：此处必须引用
            // 同一常量，两处字面量各自维护会无声漂移。
            execution: ToolExecutionPolicy::sequential(DEFAULT_UNKNOWN_TIMEOUT_SECS),
            result_policy: ToolResultPolicy::new(true),
        }
    }
}

/// 未注册/未知工具名的兜底统一超时（秒）。
pub(super) const DEFAULT_UNKNOWN_TIMEOUT_SECS: u64 = 60;

// ── 调用可声明超时的白名单工具（generate_image / edit_image / fetch_image）──
// 默认值与声明区间的单一出处：策略表行、schema 装饰器（common.rs 的
// with_call_timeout_parameters）、设置中心默认值（db/settings.rs 的
// ToolTimeoutSettings 归一化）与 effective_timeout_secs 全部引用本组常量，
// 四处不允许出现字面量副本。

/// 图片生成/编辑的默认统一超时（秒）。与 DashScope 多模态生成的真实耗时
/// 对齐（复杂提示词 + prompt_extend 扩写常超 60s，旧值 60 会误杀合法请求）。
pub(crate) const IMAGE_TOOL_TIMEOUT_SECS: u64 = 120;
/// 图片生成/编辑每次调用可声明的超时区间（秒）。
pub(crate) const IMAGE_TOOL_CALL_TIMEOUT_RANGE: (u64, u64) = (30, 300);
/// 图片下载（fetch_image）的默认统一超时（秒）。
pub(crate) const FETCH_IMAGE_TIMEOUT_SECS: u64 = 60;
/// 图片下载每次调用可声明的超时区间（秒）。
pub(crate) const FETCH_IMAGE_CALL_TIMEOUT_RANGE: (u64, u64) = (10, 300);

/// 调度器资源声明类型（`TOOL_POLICY_TABLE` 每行一列）：声明工具在并行调度下
/// 占用的宿主资源域。调度器的 claims 层据此构造 `Claim`，不再按工具名前缀
/// 推断——资源域映射与策略表其余字段同处一行维护，新增工具必须显式选择。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ClaimResource {
    /// 文件路径域：按调用参数逐路径声明只读锁，路径解析不确定时退化为
    /// 工作区域级。仅允许只读工具声明（`policy_table_rows_are_unique_and_consistent`
    /// 守护）——claims 层对 per-file 声明固定只读，出现可写路径工具时必须用
    /// Workspace，否则同文件并行写不会被互斥。
    FilePath,
    /// 工作区域级文件域（同一或嵌套工作区内互斥，读写方向按 access 声明）。
    Workspace,
    /// 会话级共享资源（浏览器共享会话等：同一宿主会话内串行，一律写锁）。
    Session,
    /// SSH 服务器级（按调用的 server_id / ssh_profile 参数，缺参按 unknown 保守）。
    SshServer,
    /// SSH 服务器 + 本地工作区双域（sync_directory：远端目录与本地文件树
    /// 都会被改写，两侧都按写锁声明）。
    SshServerAndWorkspace,
    /// 外部全局（最保守互斥，一律写锁）。
    External,
}
