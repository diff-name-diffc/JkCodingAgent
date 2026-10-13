use std::collections::HashSet;

use serde_json::json;

use super::{
    claim_resource, dispatch_window, ClaimResource, ToolCategory, ToolSafety, ToolSpec,
    DEFAULT_DISPATCH_WINDOW_MS, READONLY_DISPATCH_WINDOW_MS, TOOL_POLICY_TABLE,
};

fn spec_for(name: &str) -> ToolSpec {
    ToolSpec::new(
        name,
        "测试工具",
        json!({ "type": "object", "properties": {} }),
    )
}

#[test]
fn read_file_is_declared_as_parallel_readonly() {
    let spec = ToolSpec::new(
        "read_file",
        "读取文件",
        json!({ "type": "object", "properties": {} }),
    );

    assert_eq!(spec.category, ToolCategory::Filesystem);
    assert!(spec.access.readonly);
    assert!(spec.execution.parallelizable);
}

#[test]
fn mcp_tools_are_conservative_by_default() {
    let spec = ToolSpec::mcp(
        "mcp__server__tool".to_string(),
        "外部工具".to_string(),
        json!({ "type": "object", "properties": {} }),
    );

    assert_eq!(spec.category, ToolCategory::Mcp);
    assert!(!spec.access.readonly);
    assert!(!spec.execution.parallelizable);
}

#[test]
fn call_sub_agent_uses_its_own_timeout_policy() {
    let spec = ToolSpec::new(
        "call_sub_agent",
        "调用子智能体",
        json!({ "type": "object", "properties": {} }),
    );

    assert_eq!(spec.category, ToolCategory::SubAgent);
    assert_eq!(spec.execution.timeout_secs, 600);
    assert!(!spec.execution.unified_timeout);
    assert!(!spec.execution.parallelizable);
}

#[test]
fn shell_tools_use_runtime_context_timeout_policy() {
    let spec = ToolSpec::new(
        "local_zsh",
        "执行命令",
        json!({ "type": "object", "properties": {} }),
    );

    assert_eq!(spec.category, ToolCategory::Shell);
    assert!(!spec.execution.unified_timeout);
    assert!(!spec.execution.parallelizable);
}

#[test]
fn unknown_tool_falls_back_to_fail_closed_policy() {
    let spec = spec_for("totally_unknown_tool");

    assert_eq!(spec.category, ToolCategory::Other);
    // fail-closed：按只读 + 工作区内 + 需审查 + 串行处理，
    // 绝不落入「Safe + 无边界」的宽松兜底。
    assert!(spec.access.readonly);
    assert!(spec.access.workspace_bound);
    assert!(!spec.access.requires_network);
    assert!(!spec.access.mutates_filesystem);
    assert!(!spec.access.mutates_external_state);
    assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    assert!(!spec.execution.parallelizable);
    assert!(spec.execution.unified_timeout);
    assert!(!spec.result_policy.default_compress);
}

#[test]
fn local_zsh_declares_full_effects_and_review_required() {
    let spec = spec_for("local_zsh");

    assert_eq!(spec.category, ToolCategory::Shell);
    assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    // 能力边界按最坏情况显式声明：可改文件、可联网、可变更外部状态。
    assert!(!spec.access.readonly);
    assert!(!spec.access.workspace_bound);
    assert!(spec.access.requires_network);
    assert!(spec.access.mutates_filesystem);
    assert!(spec.access.mutates_external_state);
    assert!(spec.result_policy.default_compress);
    // 命令类工具：压缩阈值高于内联截断线，截断兜不住才摘要。
    assert_eq!(
        spec.result_policy.force_compress_after_chars,
        super::COMMAND_FORCE_COMPRESS_AFTER_CHARS
    );
    assert!(!spec.execution.unified_timeout);
}

#[test]
fn only_browser_open_url_is_exposed_and_cancellation_is_self_managed() {
    let spec = spec_for("browser_open_url");
    assert_eq!(spec.category, ToolCategory::Browser);
    assert!(spec.access.requires_network);
    assert!(spec.access.mutates_external_state);
    assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    assert!(!spec.execution.parallelizable);
    assert!(!spec.execution.unified_timeout);
    assert_eq!(spec.execution.settle_ceiling_secs, 240);
    let names: Vec<_> = TOOL_POLICY_TABLE
        .iter()
        .filter(|row| row.category == ToolCategory::Browser)
        .map(|row| row.name)
        .collect();
    assert_eq!(names, vec!["browser_open_url"]);
}

#[test]
fn message_tool_is_a_notification_tool_not_shell() {
    let spec = spec_for("message");

    assert_eq!(spec.category, ToolCategory::Other);
    assert!(!spec.access.requires_network);
    assert!(!spec.access.mutates_filesystem);
    assert!(!spec.access.mutates_external_state);
    assert_eq!(spec.safety, ToolSafety::Safe);
}

#[test]
fn analyze_image_is_declared_as_safe_self_managed_network_reader() {
    let spec = spec_for("analyze_image");

    assert_eq!(spec.category, ToolCategory::Image);
    assert!(spec.access.readonly);
    assert!(spec.access.requires_network);
    assert!(!spec.access.mutates_filesystem);
    assert!(!spec.access.mutates_external_state);
    assert_eq!(spec.safety, ToolSafety::Safe);
    assert!(!spec.execution.parallelizable);
    // 多图逐张视觉调用，超时由工具自管
    assert!(!spec.execution.unified_timeout);
}

#[test]
fn fetch_image_is_safe_and_skips_broker_review() {
    let spec = spec_for("fetch_image");

    assert_eq!(spec.category, ToolCategory::Image);
    assert_eq!(spec.safety, ToolSafety::Safe);
    // 网络侧只读：下载内容经工具内魔数校验后才入库，无需 AI 审查
    assert!(spec.access.readonly);
    assert!(spec.access.requires_network);
    assert!(!spec.access.mutates_filesystem);
    assert!(!spec.access.mutates_external_state);
    assert!(!spec.review_self_managed);
    // 与 fail-closed 兜底一致：串行 + 统一超时 + 默认不压缩
    assert!(!spec.execution.parallelizable);
    assert!(spec.execution.unified_timeout);
    assert!(!spec.result_policy.default_compress);
}

#[test]
fn ssh_list_servers_is_readonly_and_safe() {
    let spec = spec_for("ssh_list_servers");

    assert_eq!(spec.category, ToolCategory::Ssh);
    assert_eq!(spec.safety, ToolSafety::Safe);
    assert!(spec.access.readonly);
    assert!(!spec.access.mutates_external_state);
    assert!(spec.execution.parallelizable);
    assert!(!spec.review_self_managed);
}

#[test]
fn ssh_exec_is_review_required_and_self_managed() {
    let spec = spec_for("ssh_exec");

    assert_eq!(spec.category, ToolCategory::Ssh);
    assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    assert!(spec.access.mutates_external_state);
    // 压缩默认关闭：由模型显式声明；声明后也只在命令类高阈值之上才摘要。
    assert!(!spec.result_policy.default_compress);
    assert_eq!(
        spec.result_policy.force_compress_after_chars,
        super::COMMAND_FORCE_COMPRESS_AFTER_CHARS
    );
    // 审查由工具内部带服务器上下文自管；超时按每次调用参数自管（≤300s）
    assert!(spec.review_self_managed);
    assert!(!spec.execution.unified_timeout);
}

#[test]
fn tmux_install_reuses_ssh_execution_policy() {
    let spec = spec_for("ssh_tmux_install");
    assert_eq!(spec.category, ToolCategory::Ssh);
    assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    assert!(spec.access.mutates_external_state);
    assert!(spec.review_self_managed);
    assert!(!spec.execution.parallelizable);
    assert!(!spec.execution.unified_timeout);
    assert_eq!(spec.execution.timeout_secs, 300);
    assert_eq!(spec.execution.settle_ceiling_secs, 600);
    assert_eq!(claim_resource("ssh_tmux_install"), ClaimResource::SshServer);
}

#[test]
fn ssh_memo_tools_are_safe_with_self_managed_write_set() {
    let read = spec_for("ssh_memo_read");
    assert_eq!(read.category, ToolCategory::Ssh);
    assert_eq!(read.safety, ToolSafety::Safe);
    assert!(read.access.readonly);
    assert!(read.execution.parallelizable);

    for name in ["ssh_memo_upsert", "ssh_memo_delete"] {
        let spec = spec_for(name);
        assert_eq!(spec.category, ToolCategory::Ssh);
        assert_eq!(spec.safety, ToolSafety::Safe);
        // 写集固定在应用自管的备忘录目录，由工具自约束（见策略表注释）：
        // 不得声明 mutates_filesystem，否则会被写收窄授权
        // （子智能体/工作流节点 expectedFiles）fail-closed 拒绝。
        assert!(!spec.access.readonly);
        assert!(!spec.access.mutates_filesystem);
        assert!(!spec.access.mutates_external_state);
        assert!(!spec.access.requires_network);
        assert!(!spec.access.workspace_bound);
        assert!(!spec.execution.parallelizable);
        assert!(!spec.result_policy.default_compress);
    }
}

#[test]
fn command_tools_manage_their_own_review() {
    for name in [
        "local_zsh",
        "ssh_exec",
        "ssh_tmux_install",
        "sync_directory",
    ] {
        let spec = spec_for(name);

        assert!(spec.review_self_managed, "{name} 应自管安全审查");
        assert_eq!(spec.safety, ToolSafety::ReviewRequired);
    }
    // 其他 ReviewRequired 工具仍由 broker 做通用审查
    let browser_tool = spec_for("browser_open_url");
    assert!(!browser_tool.review_self_managed);
}

#[test]
fn policy_table_rows_are_unique_and_consistent() {
    let mut seen = HashSet::new();
    for row in TOOL_POLICY_TABLE {
        assert!(seen.insert(row.name), "策略表存在重复工具名：{}", row.name);
        // 并行调度仅允许真正的只读工具。
        if row.parallel_readonly {
            assert!(row.access.readonly, "{} 声明并行但非只读", row.name);
            assert!(
                !row.access.mutates_filesystem,
                "{} 声明并行但会写文件",
                row.name
            );
        }
        // 统一超时为 0 表示「不设限制」，已知工具必须给出明确正值。
        assert!(row.timeout_secs > 0, "{} 超时配置为 0", row.name);
        // 安全等级与能力声明的一致性：会变更外部状态/文件系统的工具不允许 Safe 之外
        // 的宽松组合（此处仅断言写/外部效应工具非 Safe 即需审查）。
        if row.access.mutates_external_state || row.access.mutates_filesystem {
            assert!(
                row.safety != ToolSafety::Safe || row.access.workspace_bound,
                "{} 有外部效应但既非 Safe 也非工作区内",
                row.name
            );
        }
        // FilePath 资源域只允许只读工具：claims 层对 per-file 声明固定只读锁，
        // 可写路径工具必须用 Workspace（否则同文件并行写不会被互斥）。
        if row.resource == ClaimResource::FilePath {
            assert!(
                row.access.readonly,
                "{} 声明 FilePath 资源域但非只读，应改用 Workspace",
                row.name
            );
        }
    }
}

/// 资源列与旧 claims 前缀推断语义的抽样锚定：浏览器/画布=会话级，
/// SSH 族=服务器级（sync_directory 双域），文件/搜索=路径域，命令=工作区域，
/// 其余=外部全局。
#[test]
fn resource_column_matches_domain_expectations() {
    assert_eq!(claim_resource("read_file"), ClaimResource::FilePath);
    assert_eq!(claim_resource("grep"), ClaimResource::FilePath);
    assert_eq!(claim_resource("local_zsh"), ClaimResource::Workspace);
    assert_eq!(claim_resource("browser_open_url"), ClaimResource::Session);
    assert_eq!(claim_resource("architecture_run"), ClaimResource::Session);
    assert_eq!(claim_resource("ssh_list_servers"), ClaimResource::SshServer);
    assert_eq!(
        claim_resource("sync_directory"),
        ClaimResource::SshServerAndWorkspace
    );
    assert_eq!(claim_resource("message"), ClaimResource::External);
    assert_eq!(claim_resource("workflow_get"), ClaimResource::External);
    // 未登记名字 fail-closed。
    assert_eq!(claim_resource("write_file"), ClaimResource::External);
}

/// 派发窗口列：只读并行工具放宽窗口（扇出批次整体内联交付），
/// 其余工具一律默认短窗口；未登记名 fail-closed 回退默认值。
#[test]
fn dispatch_window_column_matches_parallel_readonly() {
    for row in TOOL_POLICY_TABLE {
        let expected = if row.parallel_readonly {
            READONLY_DISPATCH_WINDOW_MS
        } else {
            DEFAULT_DISPATCH_WINDOW_MS
        };
        assert_eq!(
            row.dispatch_window_ms, expected,
            "{}：派发窗口与并行只读预设不一致",
            row.name
        );
    }
    assert_eq!(
        dispatch_window("read_file"),
        std::time::Duration::from_millis(READONLY_DISPATCH_WINDOW_MS)
    );
    assert_eq!(
        dispatch_window("local_zsh"),
        std::time::Duration::from_millis(DEFAULT_DISPATCH_WINDOW_MS)
    );
    assert_eq!(
        dispatch_window("no_such_tool"),
        std::time::Duration::from_millis(DEFAULT_DISPATCH_WINDOW_MS)
    );
}

/// 自管工具的兜底上限（策略层的最后防线）必须在策略表里显式登记：
/// 新增自管工具忘了登记 `self_managed_settle_ceiling_secs` 会在这里失败；
/// 统一超时工具不得带兜底上限。
#[test]
fn self_managed_tools_declare_settle_ceiling() {
    for row in TOOL_POLICY_TABLE {
        let spec = ToolSpec::new(row.name, "测试工具", json!({ "type": "object" }));
        if row.self_managed_timeout {
            assert!(
                spec.execution.settle_ceiling_secs > spec.execution.timeout_secs,
                "{}：兜底上限必须大于声明预算（当前 {} vs {}）",
                row.name,
                spec.execution.settle_ceiling_secs,
                spec.execution.timeout_secs
            );
            assert!(
                spec.execution.cancellable,
                "{}：自管工具必须消费取消信号（cancellable）",
                row.name
            );
        } else {
            assert_eq!(
                spec.execution.settle_ceiling_secs, 0,
                "{}：统一超时工具不应带兜底上限",
                row.name
            );
        }
    }
}

/// 调用可声明超时的白名单工具必须：在策略表内、走统一超时（自管工具
/// 不参与）、表默认落在声明区间内（否则 effective_timeout_secs 的夹紧
/// 会把表默认悄悄改掉，白名单形同虚设）。
#[test]
fn call_timeout_tools_are_unified_and_defaulted() {
    for (name, (min, max)) in [
        ("generate_image", super::IMAGE_TOOL_CALL_TIMEOUT_RANGE),
        ("edit_image", super::IMAGE_TOOL_CALL_TIMEOUT_RANGE),
        ("fetch_image", super::FETCH_IMAGE_CALL_TIMEOUT_RANGE),
    ] {
        let row = TOOL_POLICY_TABLE
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("{name}：白名单工具必须在策略表登记"));
        assert!(
            !row.self_managed_timeout,
            "{name}：白名单仅限统一超时工具（自管工具预算在工具内自管）"
        );
        assert!(
            row.timeout_secs >= min && row.timeout_secs <= max,
            "{name}：表默认 {} 必须落在声明区间 [{min}, {max}] 内",
            row.timeout_secs
        );
    }
}

/// 有效超时解析：声明优先于用户默认、用户默认优先于表默认、区间双向
/// 夹紧、白名单外工具恒为表值（声明/用户默认均被忽略）。
#[test]
fn effective_timeout_secs_resolution_order_and_clamping() {
    use super::effective_timeout_secs;
    // 声明 > 用户默认 > 表默认。
    assert_eq!(
        effective_timeout_secs("generate_image", Some(90), Some(200)),
        90
    );
    assert_eq!(
        effective_timeout_secs("generate_image", None, Some(200)),
        200
    );
    assert_eq!(
        effective_timeout_secs("generate_image", None, None),
        super::IMAGE_TOOL_TIMEOUT_SECS
    );
    // 区间夹紧（声明与用户默认同样受限）。
    assert_eq!(effective_timeout_secs("generate_image", Some(5), None), 30);
    assert_eq!(
        effective_timeout_secs("generate_image", Some(999), None),
        300
    );
    assert_eq!(effective_timeout_secs("fetch_image", None, Some(1)), 10);
    // 白名单外：恒为表值，声明/用户默认不生效。
    assert_eq!(
        effective_timeout_secs("read_file", Some(300), Some(300)),
        30
    );
    assert_eq!(
        effective_timeout_secs("ssh_exec", Some(300), Some(300)),
        300
    );
    // 未登记名走 fail-closed 兜底值。
    assert_eq!(effective_timeout_secs("no_such_tool", None, None), 60);
}
