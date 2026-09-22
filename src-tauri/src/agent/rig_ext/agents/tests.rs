//! 普通聊天 Agent 装配的端到端测试：本地 mock OpenAI 兼容 HTTP 端点
//!（真实 rig `CompletionsClient` + SSE 解析）+ 真实临时 `DispatcherDb`。
//!
//! 覆盖 `RigPlainChatAgent::run_turn` 全链路：设置 → 槽位规格 → 模型 →
//! 工具面装配 → 历史加载 → rig 运行循环 → 事件通道 → 消息落库。

use std::net::SocketAddr;
use std::sync::Arc;

use rig::tool::PortableDynamicTool;
use tauri::ipc::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::plain_chat::{ChatTurnRequest, RigPlainChatAgent};
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{AgentContext, AhaSettingsV2, DispatcherDb};
use crate::mcp::McpRegistry;
use crate::ssh_tool::SshSessionManager;

/// 一次 mock 请求的观测结果。
#[derive(Default)]
struct MockObservation {
    requests: Vec<serde_json::Value>,
}

/// 启动一个极简 OpenAI 兼容端点：对每个请求返回一段固定 SSE 流，
/// 并把请求体（JSON）记录到 `observation`。
async fn spawn_mock_endpoint(
    observation: Arc<parking_lot::Mutex<MockObservation>>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let observation = Arc::clone(&observation);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 64 * 1024];
                let mut read = 0usize;
                // 读到请求体完整为止（简化：一次 read 后按 Content-Length 补齐）。
                loop {
                    let Ok(n) = socket.read(&mut buffer[read..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    read += n;
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let Some(header_end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let content_length = text
                        .lines()
                        .find_map(|line| {
                            let line = line.to_ascii_lowercase();
                            line.strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    let body_start = header_end + 4;
                    if read < body_start + content_length {
                        continue;
                    }
                    let body = &text[body_start..body_start + content_length];
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                        observation.lock().requests.push(json);
                    }
                    let chunks = [
                        r#"{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":0,"model":"mock-chat","choices":[{"index":0,"delta":{"role":"assistant","content":"你好，"}}],"usage":null}"#,
                        r#"{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":0,"model":"mock-chat","choices":[{"index":0,"delta":{"content":"这是一个"},"finish_reason":null}],"usage":null}"#,
                        r#"{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":0,"model":"mock-chat","choices":[{"index":0,"delta":{"content":"端到端回答。"},"finish_reason":null}],"usage":null}"#,
                        r#"{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":0,"model":"mock-chat","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":5,"total_tokens":12}}"#,
                    ];
                    let mut payload = String::new();
                    for chunk in chunks {
                        payload.push_str("data: ");
                        payload.push_str(chunk);
                        payload.push_str("\n\n");
                    }
                    payload.push_str("data: [DONE]\n\n");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
            });
        }
    });
    addr
}

fn test_config(api_base: String) -> DispatcherAgentConfig {
    DispatcherAgentConfig {
        root_dir: std::env::temp_dir(),
        db_path: std::path::PathBuf::new(),
        api_key: "test-key".to_string(),
        api_base,
        model: "mock-chat".to_string(),
        summary_model: "mock-chat".to_string(),
        vision_model: String::new(),
        max_tokens: Some(128),
        temperature: 0.0,
        max_tool_iterations: 4,
        exec_timeout_secs: 30,
        restrict_to_workspace: false,
        context_debug: false,
    }
}

#[tokio::test]
async fn chat_turn_streams_answer_and_persists_messages() {
    let observation = Arc::new(parking_lot::Mutex::new(MockObservation::default()));
    let addr = spawn_mock_endpoint(Arc::clone(&observation)).await;

    let temp_dir = std::env::temp_dir().join(format!("rig-chat-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let db = DispatcherDb::new(temp_dir.join("jkbot.sqlite3")).expect("open temp db");
    let session = db
        .create_chat_session("端到端测试", None)
        .expect("create session");

    let config = test_config(format!("http://{addr}/v1"));
    let mut agent = RigPlainChatAgent::new(
        config,
        McpRegistry::new(db.clone()),
        SshSessionManager::new(db.pool()),
        None,
    );
    agent.apply_settings_v2(&AhaSettingsV2::default(), AgentContext::Chat);
    assert!(agent.is_configured());

    // 事件捕获：记录事件标签与正文增量。
    let tags = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let deltas = Arc::new(parking_lot::Mutex::new(String::new()));
    let tags_for_channel = Arc::clone(&tags);
    let deltas_for_channel = Arc::clone(&deltas);
    let on_event = Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        let tag = value
            .get("event")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default()
            .to_string();
        if tag == "assistantDelta" {
            if let Some(delta) = value
                .get("data")
                .and_then(|data| data.get("delta"))
                .and_then(|delta| delta.as_str())
            {
                deltas_for_channel.lock().push_str(delta);
            }
        }
        tags_for_channel.lock().push(tag);
        Ok(())
    });

    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let reply = agent
        .run_turn(ChatTurnRequest {
            db: &db,
            workspace_id: &session.id,
            user_segments_json:
                r#"[{"type":"text","id":"seg-1","text":"你好"}]"#.to_string(),
            on_event,
            cancel_rx,
        })
        .await
        .expect("聊天轮次应收口成功");

    // 模型侧：真实 HTTP 请求已发出，且带上了我们的模型名与用户消息。
    let requests = observation.lock().requests.clone();
    assert_eq!(requests.len(), 1, "应发出一次模型请求");
    assert_eq!(requests[0]["model"], "mock-chat");
    let messages = requests[0]["messages"].as_array().expect("messages");
    assert!(
        messages.iter().any(|message| message["role"] == "user"),
        "请求应携带用户消息：{}",
        requests[0]
    );

    // 收口正文与落库形状。
    assert_eq!(reply.plain_text().trim(), "你好，这是一个端到端回答。");
    let messages = db
        .list_visible_messages(&session.id)
        .expect("列出会话消息");
    let roles = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant"]);
    assert_eq!(
        messages[1].plain_text().trim(),
        "你好，这是一个端到端回答。"
    );

    // 事件链完整且增量拼接为正文。
    let tags = tags.lock().clone();
    for expected in [
        "started",
        "userMessage",
        "assistantStarted",
        "assistantDelta",
        "assistantMessage",
        "finished",
    ] {
        assert!(tags.iter().any(|tag| tag == expected), "缺少事件 {expected}：{tags:?}");
    }
    assert_eq!(deltas.lock().as_str(), "你好，这是一个端到端回答。");

    // 用量落库（primary 来源）。
    let usage_rows = db
        .list_session_token_usage(&session.id)
        .expect("读取用量");
    assert!(
        usage_rows
            .iter()
            .any(|row| row.model == "mock-chat" && row.prompt_tokens == 7),
        "应记录模型用量：{usage_rows:?}"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// 工具清单与定义装配的纯单元校验（不依赖 HTTP）：清单含全部候选工具
/// （允许列表过滤由 `retain_allowed_tools` 在每轮装配期承担），
/// 无子智能体时不出现 call_sub_agent，MCP 工具不因空名单出现。
#[tokio::test]
async fn static_catalog_lists_candidate_tools() {
    let temp_dir = std::env::temp_dir().join(format!("rig-chat-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let db = DispatcherDb::new(temp_dir.join("jkbot.sqlite3")).expect("open temp db");
    let config = test_config("http://127.0.0.1:1/v1".to_string());
    let mut agent = RigPlainChatAgent::new(
        config,
        McpRegistry::new(db.clone()),
        SshSessionManager::new(db.pool()),
        None,
    );
    agent.apply_settings_v2(&AhaSettingsV2::default(), AgentContext::Chat);

    // 设置页清单是「全部候选工具」（不做允许列表过滤——过滤发生在每轮装配期，
    // 由 `retain_allowed_tools` 承担，其行为见 allowlist 子模块用例）。
    agent.apply_settings_v2(&AhaSettingsV2::default(), AgentContext::Chat);
    let names = agent.static_tool_catalog();
    let tool_names = names.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>();
    assert!(tool_names.contains(&"local_zsh"));
    assert!(tool_names.contains(&"ssh_exec"));
    assert!(!tool_names.contains(&"call_sub_agent"));
    assert!(!tool_names.iter().any(|name| name.starts_with("mcp__")));

    // 工具定义可用（描述非空，schema 有效）。
    let tool = PortableDynamicTool::new(
        "probe",
        "探针",
        serde_json::json!({"type": "object"}),
        |_args| Box::pin(async { Ok(rig::tool::ToolOutput::text("ok")) }),
    );
    assert!(!tool.definition().description.is_empty());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ─── 工具面与目录名的纯函数用例（自 `agents/mod.rs` 并入） ────────────────

mod allowlist {
    use super::super::{retain_allowed_tools, session_workspace_dir_name};
    use crate::agent::rig_ext::tools::spec::ToolSpec;
    use rig::tool::PortableDynamicTool;

    fn tool(name: &str) -> PortableDynamicTool {
        PortableDynamicTool::new(
            name,
            "测试工具",
            serde_json::json!({"type": "object", "properties": {}}),
            |_args| Box::pin(async { Ok(rig::tool::ToolOutput::text("ok")) }),
        )
    }

    fn names(tools: &[PortableDynamicTool]) -> Vec<&str> {
        tools.iter().map(PortableDynamicTool::name).collect()
    }

    #[test]
    fn empty_allowlist_keeps_builtins_and_drops_all_mcp() {
        let tools = vec![
            tool("local_zsh"),
            tool("browser_read_text"),
            tool("mcp__srv__list"),
        ];
        let filtered = retain_allowed_tools(tools, &[], false);
        assert_eq!(names(&filtered), vec!["local_zsh", "browser_read_text"]);
    }

    #[test]
    fn mcp_tools_require_explicit_allowlist_entry() {
        let tools = vec![
            tool("local_zsh"),
            tool("browser_read_text"),
            tool("mcp__srv__a"),
            tool("mcp__srv__b"),
        ];
        let configured = vec!["browser_read_text".to_string(), "mcp__srv__a".to_string()];
        let filtered = retain_allowed_tools(tools, &configured, false);
        assert_eq!(names(&filtered), vec!["browser_read_text", "mcp__srv__a"]);
    }

    #[test]
    fn sub_agent_tools_are_exempt_only_for_builtin_branch() {
        let tools = vec![
            tool("browser_read_text"),
            tool("list_sub_agents"),
            tool("call_sub_agent"),
            tool("mcp__srv__a"),
        ];
        let configured = vec!["browser_read_text".to_string()];
        let filtered = retain_allowed_tools(tools, &configured, true);
        assert_eq!(
            names(&filtered),
            vec!["browser_read_text", "list_sub_agents", "call_sub_agent"]
        );
    }

    #[test]
    fn session_workspace_dir_name_is_safe_and_deterministic() {
        assert_eq!(session_workspace_dir_name("abc-123_XYZ"), "abc-123_XYZ");
        let dotted = session_workspace_dir_name("../etc");
        assert!(!dotted.contains("..") && !dotted.contains('/'));
        assert_eq!(dotted, session_workspace_dir_name("../etc"));
        assert_ne!(dotted, session_workspace_dir_name("a_b"));
        let blank = session_workspace_dir_name("  ");
        assert!(blank.starts_with("session-"));
    }

    #[test]
    fn result_policies_come_from_the_spec_table() {
        let policies = super::super::tool_result_policies_from_specs();
        let value = |name: &str| {
            policies
                .iter()
                .find(|(tool, _)| tool == name)
                .map(|(_, policy)| *policy)
        };
        let local_zsh = value("local_zsh").expect("local_zsh 在策略表中");
        assert!(local_zsh.default_compress);
        assert_eq!(
            local_zsh.force_compress_after_chars,
            crate::agent::rig_ext::tools::spec::COMMAND_FORCE_COMPRESS_AFTER_CHARS
        );
        let read_file = value("read_file").expect("read_file 在策略表中");
        assert!(!read_file.default_compress);
        // 策略表是台账/审查元数据的唯一来源：未收录名字走 fail-closed。
        assert!(!crate::agent::rig_ext::tools::spec::is_registered_tool_name("ghost"));
        assert!(!ToolSpec::new("ghost", "", serde_json::json!({})).review_self_managed);
    }
}
