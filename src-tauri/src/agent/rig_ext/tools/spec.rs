pub use contracts::*;
use registry::{ToolPolicyRow, TOOL_POLICY_TABLE};

mod contracts;
mod registry;

struct ToolProfile {
    category: ToolCategory,
    access: ToolAccess,
    safety: ToolSafety,
    review_self_managed: bool,
    execution: ToolExecutionPolicy,
    result_policy: ToolResultPolicy,
}

/// 在工具内部自管安全审查的命令类工具（与 TOOL_POLICY_TABLE 同处一个文件维护）。
/// 它们的审查携带完整目标环境上下文（目标服务器 / 执行目录、stdin、服务器级
/// 审查开关），比 broker 的通用 JSON 参数审查更准确；broker 必须让位，
/// 避免同一调用出现两套结论。新增此类工具时在此登记。
const SELF_REVIEWED_TOOLS: &[&str] = &[
    "local_zsh",
    "ssh_exec",
    "ssh_tmux_install",
    "sync_directory",
    "ssh_term_send",
];

fn lookup_policy(name: &str) -> Option<&'static ToolPolicyRow> {
    TOOL_POLICY_TABLE.iter().find(|row| row.name == name)
}

/// 该工具在策略表中是否声明为只读可并行。未登记的名字按不可并行（fail-closed）。
/// `run_tool_program` 的调度器用它分类绑定调用，不在程序源码里拒绝。
pub(crate) fn supports_parallel_readonly(name: &str) -> bool {
    lookup_policy(name).is_some_and(|row| row.parallel_readonly)
}

/// 工具的调度器资源声明类型。未登记的名字 fail-closed 按 External 处理
///（claims 层对未登记名字已提前按 External 写锁兜底，此返回值仅供防御）。
pub(crate) fn claim_resource(name: &str) -> ClaimResource {
    lookup_policy(name).map_or(ClaimResource::External, |row| row.resource)
}

/// 工具的派发直返窗口：只读快工具放宽窗口让扇出批次整体内联交付，
/// 其余工具（含未登记名，fail-closed）用默认短窗口快速转后台。
pub(crate) fn dispatch_window(name: &str) -> std::time::Duration {
    let ms = lookup_policy(name).map_or(DEFAULT_DISPATCH_WINDOW_MS, |row| row.dispatch_window_ms);
    std::time::Duration::from_millis(ms)
}

/// 该工具名是否在策略表中登记（rig 运行时的台账审计用：
/// 未登记的名字按「模型幻觉」标记 `registered=false`，不落策略字段）。
pub fn is_registered_tool_name(name: &str) -> bool {
    lookup_policy(name).is_some()
}

/// 策略表登记的全部工具名（rig 运行时按名派生结果策略/审计元数据的来源）。
pub fn registered_tool_names() -> Vec<&'static str> {
    TOOL_POLICY_TABLE.iter().map(|row| row.name).collect()
}

impl ToolProfile {
    fn from_name(name: &str) -> Self {
        match lookup_policy(name) {
            Some(row) => Self::from_row(row),
            // fail-closed：未知/未注册工具名一律保守处理，杜绝 fail-open。
            None => Self::fail_closed(),
        }
    }

    fn from_row(row: &'static ToolPolicyRow) -> Self {
        let execution = if row.self_managed_timeout {
            ToolExecutionPolicy::tool_managed_timeout(
                row.timeout_secs,
                self_managed_settle_ceiling_secs(row.name),
            )
        } else if row.parallel_readonly {
            ToolExecutionPolicy::parallel_readonly(row.timeout_secs)
        } else {
            ToolExecutionPolicy::sequential(row.timeout_secs)
        };
        Self {
            category: row.category,
            access: row.access,
            safety: row.safety,
            review_self_managed: SELF_REVIEWED_TOOLS.contains(&row.name),
            execution,
            result_policy: ToolResultPolicy::new(row.default_compress)
                .with_force_compress_after_chars(row.force_compress_after_chars),
        }
    }

    /// fail-closed 兜底策略：未知（未注册/幻觉）工具名按「只读 + 工作区内 +
    /// 需审查 + 串行执行」的最保守组合处理，确保任何未知工具都不会被标记为
    /// Safe 而绕过审查门禁，也不会被误判为可并行。
    fn fail_closed() -> Self {
        Self {
            category: ToolCategory::Other,
            access: ToolAccess::READONLY_WORKSPACE,
            safety: ToolSafety::ReviewRequired,
            review_self_managed: false,
            execution: ToolExecutionPolicy::sequential(DEFAULT_UNKNOWN_TIMEOUT_SECS),
            result_policy: ToolResultPolicy::new(false),
        }
    }
}

/// 调用可声明超时的工具白名单：返回 (最小, 最大) 秒。仅**统一超时**工具可
/// 登记（自管工具的预算在工具内部自管，策略层不夹紧单次值）；schema 侧的
/// minimum/maximum（common.rs 的 `with_call_timeout_parameters`）与策略层
/// 夹紧共用本区间，两处不允许各自写字面量。
///
/// 集中一处而非写进主策略表，模式同 `self_managed_settle_ceiling_secs`
///（把少数工具的特例放在一起横向比较，不给主表加列）。
/// 新增白名单工具须同步五处：本 match、`effective_timeout_secs` 的语义依赖、
/// 工具 schema 注入、设置中心默认值（db/settings.rs 的 ToolTimeoutSettings）、
/// 用户默认下传路由（tools/deps.rs 的 ToolTimeoutDefaults/for_tool——漏改
/// 该处用户配置默认会静默失效，`deps` 模块测试守护路由完整性）与
/// `call_timeout_tools_are_unified_and_defaulted` 守护测试。
fn call_timeout_range(name: &str) -> Option<(u64, u64)> {
    match name {
        "generate_image" | "edit_image" => Some(IMAGE_TOOL_CALL_TIMEOUT_RANGE),
        "fetch_image" => Some(FETCH_IMAGE_CALL_TIMEOUT_RANGE),
        _ => None,
    }
}

/// 白名单工具的有效超时（秒）：调用声明 > 用户配置默认 > 策略表默认，
/// 结果夹紧到声明区间；白名单外/未登记工具恒为表值（含 0 = 不设统一超时
/// 的语义保持不变）。
///
/// 策略层 deadline（`loop/app_policy.rs`）与工具内部 HTTP/下载预算**必须共用
/// 本函数**：两处各自计算会出现「用户配了 300 却在 120 被 HTTP 层掐死」的
/// 口径漂移。`declared` 来自经 schema 校验的调用参数（未声明为 None——
/// schema 不设 default 键正是为了让用户配置默认有机会生效）。
pub(crate) fn effective_timeout_secs(
    name: &str,
    declared: Option<u64>,
    user_default: Option<u64>,
) -> u64 {
    let table_secs =
        lookup_policy(name).map_or(DEFAULT_UNKNOWN_TIMEOUT_SECS, |row| row.timeout_secs);
    match call_timeout_range(name) {
        Some((min, max)) => declared
            .or(user_default)
            .unwrap_or(table_secs)
            .clamp(min, max),
        None => table_secs,
    }
}

/// 自管超时工具的兜底上限（秒）：工具自身预算之外，策略层等待结算的最后防线。
///
/// 集中一处而非写进主策略表，是为了把自管工具的「最坏合法预算」放在一起横向比较，
/// 也避免给主表 50 余行都加参数。取值原则：**不得小于该工具的最坏合法预算**（宁可宽），
/// 只在工具终止路径卡死时才会触发（到点行为见 `loop/app_policy.rs` 的 ceiling 分支）。
///
/// 返回 0 表示未登记：新增自管工具若忘了在这里登记，`self_managed_tools_declare_settle_ceiling`
/// 测试会失败。
fn self_managed_settle_ceiling_secs(name: &str) -> u64 {
    match name {
        // config `exec_timeout_secs` 默认 60（非前端可配），留 10× 余量
        "local_zsh" => 600,
        // 单命令参数上限 300 + 交互/静默容忍与 channel drain
        "ssh_exec" | "ssh_tmux_install" => 600,
        // rsync 无硬上限
        "sync_directory" => 900,
        // 多图批量 × 单图超时（声明 1500），留 2× 余量
        "analyze_image" => 3000,
        // 子智能体配置上限 MAX_TIMEOUT_SECS = 3600 + 收敛窗口
        "call_sub_agent" => 3900,
        // 环境准备/打开 URL 180 秒 + 进程树清理宽限
        "browser_open_url" => 240,
        // 程序 wall-time 120 + 在途叶子最长 300 + drain
        "run_tool_program" => 900,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
