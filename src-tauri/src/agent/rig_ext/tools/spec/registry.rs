use super::*;

pub(super) struct ToolPolicyRow {
    pub(super) name: &'static str,
    pub(super) category: ToolCategory,
    pub(super) access: ToolAccess,
    pub(super) safety: ToolSafety,
    pub(super) timeout_secs: u64,
    pub(super) default_compress: bool,
    pub(super) force_compress_after_chars: usize,
    pub(super) parallel_readonly: bool,
    pub(super) self_managed_timeout: bool,
    pub(super) resource: ClaimResource,
    pub(super) dispatch_window_ms: u64,
}

struct ToolPolicyOptions {
    default_compress: bool,
    parallel_readonly: bool,
    self_managed_timeout: bool,
    force_compress_after_chars: usize,
    dispatch_window_ms: u64,
}

impl ToolPolicyOptions {
    const SERIAL: Self = Self {
        default_compress: false,
        parallel_readonly: false,
        self_managed_timeout: false,
        force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        dispatch_window_ms: DEFAULT_DISPATCH_WINDOW_MS,
    };
    const PARALLEL_READONLY: Self = Self {
        default_compress: false,
        parallel_readonly: true,
        self_managed_timeout: false,
        force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        dispatch_window_ms: READONLY_DISPATCH_WINDOW_MS,
    };
    const SELF_MANAGED: Self = Self {
        default_compress: false,
        parallel_readonly: false,
        self_managed_timeout: true,
        force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        dispatch_window_ms: DEFAULT_DISPATCH_WINDOW_MS,
    };
    /// 命令执行类工具（exec / local_zsh）：默认开启压缩，超时自管，
    /// 压缩阈值用命令类高阈值（内联截断兜不住的输出才值得摘要）。
    const COMPRESSED_SELF_MANAGED: Self = Self {
        default_compress: true,
        parallel_readonly: false,
        self_managed_timeout: true,
        force_compress_after_chars: COMMAND_FORCE_COMPRESS_AFTER_CHARS,
        dispatch_window_ms: DEFAULT_DISPATCH_WINDOW_MS,
    };
    /// 命令执行类工具的「默认不压缩」预设（ssh_exec）：命令输出常需逐字核对，
    /// 压缩须由模型显式声明；声明后也只在超过命令类高阈值时才真正摘要。
    const UNCOMPRESSED_SELF_MANAGED: Self = Self {
        default_compress: false,
        parallel_readonly: false,
        self_managed_timeout: true,
        force_compress_after_chars: COMMAND_FORCE_COMPRESS_AFTER_CHARS,
        dispatch_window_ms: DEFAULT_DISPATCH_WINDOW_MS,
    };
}

const fn policy_row(
    name: &'static str,
    category: ToolCategory,
    access: ToolAccess,
    safety: ToolSafety,
    timeout_secs: u64,
    options: ToolPolicyOptions,
    resource: ClaimResource,
) -> ToolPolicyRow {
    ToolPolicyRow {
        name,
        category,
        access,
        safety,
        timeout_secs,
        default_compress: options.default_compress,
        force_compress_after_chars: options.force_compress_after_chars,
        parallel_readonly: options.parallel_readonly,
        self_managed_timeout: options.self_managed_timeout,
        resource,
        dispatch_window_ms: options.dispatch_window_ms,
    }
}

/// 已知工具策略表（唯一事实来源）。
///
/// 约定：
/// - resource 列是调度器资源声明的唯一来源（见 `ClaimResource`），claims 层
///   不再按工具名前缀推断资源域；
/// - 浏览器展示共用会话，与 browser-use 的执行权由 BrowserManager 再统一互斥；
/// - exec / local_zsh / ssh_exec 是命令执行类工具：ReviewRequired 且 access 按
///   最坏能力声明；安全审查由工具内部自管（见 SELF_REVIEWED_TOOLS）；
/// - ssh_list_servers 是纯只读枚举（读取应用全局配置做投影），按 Safe 声明；
/// - 表中未收录的工具名视为未知工具，走 fail-closed 兜底。
pub(super) static TOOL_POLICY_TABLE: &[ToolPolicyRow] = &[
    // ── 文件系统 ──
    policy_row(
        "read_file",
        ToolCategory::Filesystem,
        ToolAccess::READONLY_WORKSPACE,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::FilePath,
    ),
    policy_row(
        "list_dir",
        ToolCategory::Filesystem,
        ToolAccess::READONLY_WORKSPACE,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::FilePath,
    ),
    // ── 搜索 ──
    policy_row(
        "glob",
        ToolCategory::Search,
        ToolAccess::READONLY_WORKSPACE,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::FilePath,
    ),
    policy_row(
        "grep",
        ToolCategory::Search,
        ToolAccess::READONLY_WORKSPACE,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::FilePath,
    ),
    // ── 命令执行（能力边界按最坏情况声明，强制审查）──
    policy_row(
        "local_zsh",
        ToolCategory::Shell,
        ToolAccess::FULL_EFFECTS,
        ToolSafety::ReviewRequired,
        60,
        ToolPolicyOptions::COMPRESSED_SELF_MANAGED,
        ClaimResource::Workspace,
    ),
    // message 是面向用户的最终消息通知工具（映射为 ToolAction::FinalMessage），
    // 并非 shell 命令，不归入 Shell 类。
    policy_row(
        "message",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // ── 通用浏览器展示（探索由 browser-use 子智能体管理）──
    policy_row(
        "browser_open_url",
        ToolCategory::Browser,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        180,
        ToolPolicyOptions::SELF_MANAGED,
        ClaimResource::Session,
    ),
    // ── 图像生成 ──
    // 默认超时与调用可声明区间见 spec 顶部常量组（IMAGE_TOOL_*）：统一超时
    // 工具，Agent 可在调用参数中声明 timeout_secs 延长预算（见
    // call_timeout_range / effective_timeout_secs）。
    policy_row(
        "generate_image",
        ToolCategory::Image,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        IMAGE_TOOL_TIMEOUT_SECS,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "edit_image",
        ToolCategory::Image,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        IMAGE_TOOL_TIMEOUT_SECS,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // ── 图像分析 ──
    // 只读外部网络工具：读取本地/远端图片并调用视觉模型，不变更任何外部
    // 状态。单次调用内部按图片顺序逐张请求视觉模型，最坏 8 张 × 单张
    // 180s LLM 超时远超任何合理统一超时，因此超时自管（工具内逐张
    // tokio timeout + cancel_rx 检查，同 ssh_exec 的自管模式）。
    policy_row(
        "analyze_image",
        ToolCategory::Image,
        ToolAccess {
            readonly: true,
            workspace_bound: false,
            requires_network: true,
            mutates_filesystem: false,
            mutates_external_state: false,
        },
        ToolSafety::Safe,
        1500,
        ToolPolicyOptions::SELF_MANAGED,
        ClaimResource::External,
    ),
    // ── 图片下载 ──
    // fetch_image 按给定 URL 下载图片入库：网络侧只读，落盘走应用自管的
    // 聊天图片库；下载内容在工具内做魔数校验（仅 png/jpg/webp/gif 二进制，
    // HTML/SVG/文本等非图片响应直接拒绝），无需 AI 审查。慢速图源可经调用
    // 参数声明更长预算（FETCH_IMAGE_* 常量组）。
    policy_row(
        "fetch_image",
        ToolCategory::Image,
        ToolAccess {
            readonly: true,
            workspace_bound: false,
            requires_network: true,
            mutates_filesystem: false,
            mutates_external_state: false,
        },
        ToolSafety::Safe,
        FETCH_IMAGE_TIMEOUT_SECS,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // ── SSH ──
    // ssh_list_servers 只做本地配置的只读投影（不含凭据），安全声明为只读；
    // ssh_exec 按命令执行类工具的最坏能力声明，审查在工具内部自管，
    // 超时也由工具按每次调用的 timeout_secs 自管（上限 300s）。
    // 压缩默认关闭：SSH 输出（报错原文、状态值、配置片段）常需逐字核对，
    // 8000 字符内联直读；只有模型显式声明且输出超过命令类阈值时才摘要。
    policy_row(
        "ssh_list_servers",
        ToolCategory::Ssh,
        ToolAccess::READONLY_UNBOUND,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_exec",
        ToolCategory::Ssh,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        300,
        ToolPolicyOptions::UNCOMPRESSED_SELF_MANAGED,
        ClaimResource::SshServer,
    ),
    // 显式安装入口把固定脚本委托 ssh_exec，共用其审查与超时边界。
    policy_row(
        "ssh_tmux_install",
        ToolCategory::Ssh,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        300,
        ToolPolicyOptions::UNCOMPRESSED_SELF_MANAGED,
        ClaimResource::SshServer,
    ),
    // ── SSH 交互终端（ssh_term_*，设计文档 docs/ssh-term-agent-2026-10-08.md）──
    // open/send 按命令执行类工具的最坏能力声明：send 每次输入都过审查
    //（review_self_managed，同 ssh_exec）；open 的审查面在带 command 路径，
    // 裸 shell / tmux 模板命令免审（应用拼装 + 会话名白名单，注入面封闭）。
    // read/list 只读；close 关闭自开终端，效果由 term 子系统托管（同 ssh_memo
    // 写侧口径）。统一超时 60s：read 的 wait_ms ≤25s、send 内嵌审查 30s（ssh_review
    // 自带 REVIEW_TIMEOUT_SECS）均在预算内。
    policy_row(
        "ssh_term_open",
        ToolCategory::Ssh,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_term_send",
        ToolCategory::Ssh,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_term_read",
        ToolCategory::Ssh,
        ToolAccess::READONLY_UNBOUND,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_term_resize",
        ToolCategory::Ssh,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_term_close",
        ToolCategory::Ssh,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_term_list",
        ToolCategory::Ssh,
        ToolAccess::READONLY_UNBOUND,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "sync_directory",
        ToolCategory::Ssh,
        ToolAccess::EXTERNAL_EFFECTS,
        ToolSafety::ReviewRequired,
        300,
        ToolPolicyOptions::UNCOMPRESSED_SELF_MANAGED,
        ClaimResource::SshServerAndWorkspace,
    ),
    // ── SSH 运维备忘录 ──
    // 备忘录工具的读写集固定在应用自管的备忘录目录
    // （~/.jkcodingagent/ssh-memos/）：路径由 validate_server_id 白名单拼接、
    // 有全文/单段字符硬上限（超限拒绝写盘），没有任意路径选择或任意执行能力，
    // 因此写侧按 SUBSYSTEM_MANAGED 声明（同 message/submit_workflow 的「效果由
    // 对应子系统托管」语义）——写收窄授权（子智能体/工作流节点 expectedFiles）
    // 下 mutates_filesystem 工具会被 fail-closed 拒绝，而备忘录写入不属于
    // 工作区写集，也不应被工作区授权拦截。
    policy_row(
        "ssh_memo_read",
        ToolCategory::Ssh,
        ToolAccess::READONLY_UNBOUND,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::PARALLEL_READONLY,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_memo_upsert",
        ToolCategory::Ssh,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    policy_row(
        "ssh_memo_delete",
        ToolCategory::Ssh,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        30,
        ToolPolicyOptions::SERIAL,
        ClaimResource::SshServer,
    ),
    // ── 子智能体 ──
    policy_row(
        "call_sub_agent",
        ToolCategory::SubAgent,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        600,
        ToolPolicyOptions::SELF_MANAGED,
        ClaimResource::External,
    ),
    policy_row(
        "list_sub_agents",
        ToolCategory::SubAgent,
        ToolAccess::READONLY_UNBOUND,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "notify_user_progress",
        ToolCategory::SubAgent,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // ── 工作流编排协议壳 ──
    // 外层运行时只看到这一次程序调用。子步骤是程序里的绑定调用：
    // 授权以注入的数据面为准，参数在每次调用时按该工具的 JSON Schema 校验。
    // 该入口仅在项目编排器注册。
    policy_row(
        "run_tool_program",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        120,
        ToolPolicyOptions::SELF_MANAGED,
        ClaimResource::External,
    ),
    policy_row(
        "submit_workflow",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "workflow_plan_report",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // workflow_result_read：宿主拦截的执行结果回读（结论 md 全文 + 修改文件
    // 清单）。不声明 compress——LLM 压缩会把问题清单摘要掉，违背工具目的；
    // 超长结论由拦截层确定性截断（CONCLUSION_MAX_CHARS）。
    policy_row(
        "workflow_result_read",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // workflow_get / workflow_node_{update,add,delete}：宿主拦截的读感知与 draft 工作流
    // 节点级 CRUD，效果由协议处理器托管（同上 SUBSYSTEM_MANAGED 语义），
    // 与报告工具同口径。
    policy_row(
        "workflow_get",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "workflow_node_update",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "workflow_node_add",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    policy_row(
        "workflow_node_delete",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        60,
        ToolPolicyOptions::SERIAL,
        ClaimResource::External,
    ),
    // ── 架构画布 ──
    // architecture_run 的真实效应由前端画布解释器托管（同 SUBSYSTEM_MANAGED
    // 语义）；SERIAL 即 default_compress=false——执行报告里的 `chat-image://`
    // 截图引用必须原样保留给下一轮 attach_turn_tool_images，绝不走摘要压缩。
    // 超时 40s > 工具内部 20s 画布等待上限，留出事件/回传余量。
    // 资源域是会话级（画布与聊天同属一个宿主会话）。
    policy_row(
        "architecture_run",
        ToolCategory::Other,
        ToolAccess::SUBSYSTEM_MANAGED,
        ToolSafety::Safe,
        40,
        ToolPolicyOptions::SERIAL,
        ClaimResource::Session,
    ),
];
