//! ACP 节点会话的进程级端到端握手测试。
//!
//! 与 `tests.rs`（映射层纯单元）互补：这里以真实子进程 + stdio JSON-RPC
//! 跑通 `client::run_prompt_turn` 的完整链路——`examples/acp_stub_agent.rs`
//! 以 v1 协议扮演 claude-agent-acp 的契约面（modes / model 选择器、
//! session/update、session/request_permission、`_meta.quota` 计费计数），
//! 经 `process::spawn`（env_clear + 进程组守卫）拉起，并把执行器实际收到
//! 的请求转录回文件，逐项断言客户端的线上行为（协议版本、`_meta` 钉死
//! 项、bypass 模式设置、模型选择、权限应答）。agent-client-protocol
//! crate 大版本升级（如 2→3）会改变线上报文形态，常规单测无法覆盖，
//! 这里是唯一防线。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::StopReason;
use serde_json::Value;
use tauri::test::mock_app;
use tokio::sync::{mpsc, watch};

use super::client::{self, AcpSessionError};
use super::launcher::LaunchPlan;
use super::mapping::Mapper;
use super::permission_review::{PermissionReviewMaterials, WorkflowReviewShared};
use super::HandlerContext;
use crate::agent::db::settings::SshReviewConfig;
use crate::agent::workflow::harness::{PermissionMode, ResolvedNodeHarness};
use crate::agent::workflow::types::{BaseToolGroup, WorkflowNode};

/// 单轮握手测试的兜底超时：stub 为本地进程，正常毫秒级完成。
const TURN_TIMEOUT: Duration = Duration::from_secs(30);

/// 定位（必要时补建）stub 执行器。`cargo test` 对 examples 只做编译检查、
/// 不链接二进制（`--no-run` 才链接），因此缺失时按需 `cargo build --example`
/// 自愈——cargo 在执行测试前已释放目标目录构建锁，嵌套 build 不会死锁。
fn stub_agent_program() -> PathBuf {
    let release = option_env!("PROFILE") == Some("release");
    // cargo 的 profile 名（dev）与产物目录名（debug）不同，分开持有。
    let (profile, dir) = if release {
        ("release", "release")
    } else {
        ("dev", "debug")
    };
    let name = if cfg!(windows) {
        "acp_stub_agent.exe"
    } else {
        "acp_stub_agent"
    };
    let target_dir = option_env!("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
    let program = target_dir.join(dir).join("examples").join(name);
    if !program.is_file() {
        let output = std::process::Command::new(env!("CARGO"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "--example", "acp_stub_agent", "--profile", profile])
            .arg("--target-dir")
            .arg(&target_dir)
            .output()
            .expect("按需补建 stub 执行器失败：无法启动 cargo");
        assert!(
            output.status.success(),
            "按需补建 stub 执行器失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        program.is_file(),
        "仍未找到 stub 执行器（{}）：补建后应存在",
        program.display()
    );
    program
}

/// 读取 stub 转录的 JSONL（每行一个请求/事件）。
fn read_transcript(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读取转录文件失败（{}）：{error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("转录行必为合法 JSON"))
        .collect()
}

fn transcript_entry<'a>(entries: &'a [Value], method: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry["method"] == method)
        .unwrap_or_else(|| panic!("转录缺少 {method} 条目：{entries:?}"))
}

/// 构造权限审查材料：EnterPlanMode 走客户端静态放行路径，审查模型不可达
/// 也不影响本测试；保持真实构造（非 Default 的空壳）以贴近生产装配。
fn review_materials() -> Arc<PermissionReviewMaterials> {
    let node = WorkflowNode {
        id: "n1".into(),
        title: "握手验证节点".into(),
        role: "coder".into(),
        model_ref: "sonnet".into(),
        base_tool_group: BaseToolGroup::Coding,
        task: "端到端握手验证".into(),
        depends_on: Vec::new(),
        inject_state_keys: Vec::new(),
        output_key: String::new(),
        expected_files: Vec::new(),
        export_policy: Default::default(),
        use_plan_mode: false,
    };
    let shared = Arc::new(WorkflowReviewShared {
        config: SshReviewConfig::default(),
        workflow_digest: "端到端握手测试工作流摘要".into(),
    });
    Arc::new(PermissionReviewMaterials::new(shared, &node))
}

#[tokio::test]
async fn handshake_round_trip_end_to_end() {
    let guard = crate::test_util::TempDirGuard::new("acp-e2e");
    let workspace = guard.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let transcript = guard.path().join("transcript.jsonl");
    let app = mock_app();

    let mapper = Arc::new(parking_lot::Mutex::new(Mapper::new("r1", "n1", &workspace)));
    let (activity_tx, mut activity_rx) = mpsc::unbounded_channel();
    let handler = HandlerContext {
        app: app.handle().clone(),
        plan_id: "plan-1".into(),
        run_id: "r1".into(),
        workspace_id: "ws-1".into(),
        node_id: "n1".into(),
        workspace_root: workspace.clone(),
        mapper: Arc::clone(&mapper),
        activity_tx,
        review: review_materials(),
    };
    let harness = ResolvedNodeHarness {
        model_id: "sonnet".into(),
        model_label: "Sonnet".into(),
        permission_mode: PermissionMode::BypassPermissions,
    };
    let plan = LaunchPlan {
        program: stub_agent_program(),
        args: Vec::new(),
        envs: vec![(
            "ACP_STUB_TRANSCRIPT".into(),
            transcript.to_string_lossy().into_owned(),
        )],
        diagnostics: Vec::new(),
    };
    let (_cancel_tx, cancel_rx) = watch::channel(false);

    let run = tokio::time::timeout(
        TURN_TIMEOUT,
        client::run_prompt_turn(
            plan,
            workspace.clone(),
            "端到端握手".into(),
            &harness,
            handler.clone(),
            cancel_rx,
        ),
    )
    .await
    .expect("握手超时：stub 本地进程应在秒级完成");
    let turn = run.unwrap_or_else(|error| panic!("握手应成功：{error:?}"));
    assert!(matches!(turn.stop_reason, StopReason::EndTurn));

    // 结算面：执行器上报的计费口径计数归一为聚合层 usage_json。
    let usage: Value = serde_json::from_str(&turn.usage_json).unwrap();
    // prompt_tokens = input(1200) + 缓存读(30000) + 缓存写(4000)。
    assert_eq!(usage["prompt_tokens"], 35_200);
    assert_eq!(usage["completion_tokens"], 250);
    assert_eq!(usage["cached_read_tokens"], 30_000);
    assert_eq!(usage["cached_write_tokens"], 4_000);

    // 映射面：session/update 通知落进 mapper（输出/工具计数/受影响文件）。
    let (_, output, tool_call_count, affected) = mapper.lock().finish();
    assert_eq!(output, "stub 产出");
    assert_eq!(tool_call_count, 1);
    assert_eq!(affected, vec!["src/main.rs"]);

    // 权限应答的审计活动经 channel 落库侧：等待持有 sender 的副本
    // （连接 future / 权限任务）全部析构后通道关闭。
    drop(handler);
    let mut activities = Vec::new();
    while let Ok(Some(activity)) =
        tokio::time::timeout(Duration::from_secs(5), activity_rx.recv()).await
    {
        activities.push(activity);
    }
    assert!(
        activities
            .iter()
            .any(|activity| activity.title.contains("允许进入计划模式")),
        "应记录 EnterPlanMode 静态放行的审计活动：{activities:?}"
    );

    // 转录面：执行器实际收到的线上请求。
    let entries = read_transcript(&transcript);
    let initialize = transcript_entry(&entries, "initialize");
    // v1 线上形态：protocolVersion 是数字 1（非字符串）。
    assert_eq!(initialize["params"]["protocolVersion"], 1);
    let new_session = transcript_entry(&entries, "session/new");
    assert_eq!(
        new_session["params"]["_meta"]["claudeCode"]["options"]["settings"]["env"]
            ["ENABLE_TOOL_SEARCH"],
        "false",
        "session/new 必须钉死 ENABLE_TOOL_SEARCH=false"
    );
    assert_eq!(
        new_session["params"]["cwd"],
        workspace.display().to_string(),
        "session/new 的 cwd 必须是节点工作区"
    );
    let set_mode = transcript_entry(&entries, "session/set_mode");
    assert_eq!(set_mode["params"]["modeId"], "bypassPermissions");
    let set_model = transcript_entry(&entries, "session/set_config_option");
    assert!(
        serde_json::to_string(&set_model["params"])
            .unwrap()
            .contains("sonnet"),
        "模型选择必须落到 sonnet：{set_model}"
    );
    let prompt = transcript_entry(&entries, "session/prompt");
    assert_eq!(prompt["params"]["promptText"], "端到端握手");
    // EnterPlanMode 静态放行：选中一次性允许（不放大为 allow_always）。
    let outcome = transcript_entry(&entries, "session/request_permission.outcome");
    let outcome_text = outcome["outcome"].as_str().unwrap_or_default();
    assert!(
        outcome_text.contains("Selected") && outcome_text.contains("once"),
        "权限应答应为 Selected(option_id=once)：{outcome}"
    );
}

#[tokio::test]
async fn initialize_failure_fails_closed_with_diagnostics() {
    let guard = crate::test_util::TempDirGuard::new("acp-e2e-fail");
    let workspace = guard.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let app = mock_app();
    let mapper = Arc::new(parking_lot::Mutex::new(Mapper::new("r2", "n2", &workspace)));
    let (activity_tx, _activity_rx) = mpsc::unbounded_channel();
    let handler = HandlerContext {
        app: app.handle().clone(),
        plan_id: "plan-1".into(),
        run_id: "r2".into(),
        workspace_id: "ws-1".into(),
        node_id: "n2".into(),
        workspace_root: workspace.clone(),
        mapper,
        activity_tx,
        review: review_materials(),
    };
    let harness = ResolvedNodeHarness {
        model_id: "default".into(),
        model_label: "默认".into(),
        permission_mode: PermissionMode::BypassPermissions,
    };
    let plan = LaunchPlan {
        program: stub_agent_program(),
        args: Vec::new(),
        envs: vec![
            (
                "ACP_STUB_TRANSCRIPT".into(),
                guard
                    .path()
                    .join("transcript.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("ACP_STUB_MODE".into(), "crash".into()),
        ],
        diagnostics: Vec::new(),
    };
    let (_cancel_tx, cancel_rx) = watch::channel(false);

    let run = tokio::time::timeout(
        TURN_TIMEOUT,
        client::run_prompt_turn(
            plan,
            workspace.clone(),
            "不应执行".into(),
            &harness,
            handler,
            cancel_rx,
        ),
    )
    .await
    .expect("握手超时");
    match run {
        Err(AcpSessionError::Failed(message)) => {
            assert!(
                message.contains("ACP initialize 失败"),
                "错误应定位到 initialize 阶段：{message}"
            );
            assert!(
                message.contains("stub initialize 故意失败"),
                "失败应附执行器 stderr 尾部诊断：{message}"
            );
        }
        other => panic!("期望握手失败，得到：{other:?}"),
    }
}
