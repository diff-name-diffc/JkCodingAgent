use super::*;

#[test]
fn recursively_redacts_secrets() {
    let value = json!({
        "apiKey": "one",
        "nested": { "refresh_token": "two", "safe": "visible" },
        "items": [{ "authorization": "Bearer three" }]
    });
    let redacted = redact(value);
    assert_eq!(redacted["apiKey"], "***");
    assert_eq!(redacted["nested"]["refresh_token"], "***");
    assert_eq!(redacted["nested"]["safe"], "visible");
    assert_eq!(redacted["items"][0]["authorization"], "***");
}

#[test]
fn rejects_invalid_sidecar_envelopes() {
    assert!(parse_sidecar_envelope("not-json", "request", "run", "node", 0).is_err());

    let frame = |request_id: &str, run_id: &str, node_id: &str, sequence: i64| {
        json!({
            "type": "agent_event",
            "requestId": request_id,
            "runId": run_id,
            "nodeId": node_id,
            "sequence": sequence,
            "data": {},
        })
        .to_string()
    };
    assert!(parse_sidecar_envelope(
        &frame("other", "run", "node", 1),
        "request",
        "run",
        "node",
        0
    )
    .is_err());
    assert!(parse_sidecar_envelope(
        &frame("request", "other", "node", 1),
        "request",
        "run",
        "node",
        0
    )
    .is_err());
    assert!(parse_sidecar_envelope(
        &frame("request", "run", "node", 1),
        "request",
        "run",
        "node",
        1
    )
    .is_err());
    assert!(parse_sidecar_envelope(
        &json!({
            "type": "ready",
            "requestId": "sidecar",
            "sequence": 1,
            "data": { "protocolVersion": PROTOCOL_VERSION + 1 },
        })
        .to_string(),
        "request",
        "run",
        "node",
        0,
    )
    .is_err());
}

#[test]
fn accepts_only_current_protocol_ready_before_business_messages() {
    let ready = parse_sidecar_envelope(
        &json!({
            "type": "ready",
            "requestId": "sidecar",
            "sequence": 1,
            "data": { "protocolVersion": 3 },
        })
        .to_string(),
        "request",
        "run",
        "node",
        0,
    )
    .unwrap();
    assert_eq!(PROTOCOL_VERSION, 3);
    assert_eq!(ready.r#type, "ready");

    let mut ready_seen = false;
    assert!(enforce_handshake_order("agent_event", &mut ready_seen).is_err());
    enforce_handshake_order(&ready.r#type, &mut ready_seen).unwrap();
    assert!(enforce_handshake_order("agent_event", &mut ready_seen).is_ok());

    // ready 本身也必须遵守正数、单调 sequence，不能以 0 绕过边界。
    assert!(parse_sidecar_envelope(
        &json!({
            "type": "ready",
            "requestId": "sidecar",
            "sequence": 0,
            "data": { "protocolVersion": 3 },
        })
        .to_string(),
        "request",
        "run",
        "node",
        0,
    )
    .is_err());
}

#[test]
fn host_alias_and_capability_must_match_as_a_pair() {
    let tools = vec![super::super::harness::PiHostToolSpec {
        name: "exec".into(),
        runtime_name: "bash".into(),
        description: String::new(),
        parameters: json!({ "type": "object" }),
    }];
    assert!(host_tool_mapping_is_declared(&tools, "bash", "exec"));
    assert!(!host_tool_mapping_is_declared(&tools, "bash", "read_file"));
    assert!(!host_tool_mapping_is_declared(&tools, "read", "exec"));
}

#[test]
fn host_tool_result_is_bounded_on_utf8_boundary() {
    let output = "你".repeat(MAX_HOST_TOOL_RESULT_BYTES);
    let bounded = bound_host_tool_result(output);
    assert!(bounded.len() <= MAX_HOST_TOOL_RESULT_BYTES);
    assert!(bounded.contains("Graph JSONL 传输预算截断"));
    assert!(std::str::from_utf8(bounded.as_bytes()).is_ok());
}

#[test]
fn rejects_agent_event_sequence_overflow() {
    assert_eq!(activity_sequence(42).unwrap(), 420);
    assert!(activity_sequence(i64::MAX).is_err());
    assert!(activity_sequence(i64::MIN).is_err());
}

#[test]
fn context_usage_content_formats_reading_and_unknown() {
    let reading = json!({ "tokens": 55_000, "contextWindow": 128_000, "percent": 42.97 });
    assert_eq!(
        context_usage_content(&reading),
        "估算占用 43.0%（55000/128000 tokens）"
    );
    // compaction 后 tokens/percent 为 null：明确展示「重新估算中」而非 0%。
    let unknown = json!({ "tokens": null, "contextWindow": 128_000, "percent": null });
    assert_eq!(context_usage_content(&unknown), "上下文压缩后重新估算中…");
}

#[tokio::test]
async fn pending_tool_future_is_interruptible_by_cancellation() {
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    cancel_tx.send(true).unwrap();
    let mut pending = std::future::pending::<()>();
    let result = await_interruptible(
        std::pin::Pin::new(&mut pending),
        &mut cancel_rx,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;
    assert_eq!(result, Interruptible::Cancelled);
}

#[tokio::test]
async fn pending_tool_future_is_interruptible_by_deadline() {
    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let mut pending = std::future::pending::<()>();
    let result = await_interruptible(
        std::pin::Pin::new(&mut pending),
        &mut cancel_rx,
        tokio::time::Instant::now() + Duration::from_millis(10),
    )
    .await;
    assert_eq!(result, Interruptible::TimedOut);
}

#[tokio::test]
async fn dropped_cancel_sender_is_treated_as_cancellation() {
    // 发送端丢弃后 changed() 返回 Err：必须按取消返回，禁止永久 pending。
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    drop(cancel_tx);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        cancellation_requested(&mut cancel_rx),
    )
    .await;
    assert!(result.is_ok());
}

#[test]
fn redacts_secret_key_token_variants() {
    let value = json!({
        "X-Api-Key": "one",
        "client_secret": "two",
        "access_key": "three",
        "auth_token": "four",
        "db_password": "five",
        "nested": { "SIGNING_KEY": "six" },
        "name": "visible",
        "description": "also visible"
    });
    let redacted = redact(value);
    assert_eq!(redacted["X-Api-Key"], "***");
    assert_eq!(redacted["client_secret"], "***");
    assert_eq!(redacted["access_key"], "***");
    assert_eq!(redacted["auth_token"], "***");
    assert_eq!(redacted["db_password"], "***");
    assert_eq!(redacted["nested"]["SIGNING_KEY"], "***");
    assert_eq!(redacted["name"], "visible");
    assert_eq!(redacted["description"], "also visible");
}

#[test]
fn requires_ready_handshake_before_business_messages() {
    let mut ready_seen = false;
    // ready 之前的业务消息必须拒绝。
    assert!(enforce_handshake_order("agent_event", &mut ready_seen).is_err());
    assert!(enforce_handshake_order("completed", &mut ready_seen).is_err());
    assert!(!ready_seen);
    // 首个 ready 放行，重复 ready 拒绝。
    assert!(enforce_handshake_order("ready", &mut ready_seen).is_ok());
    assert!(ready_seen);
    assert!(enforce_handshake_order("ready", &mut ready_seen).is_err());
    // 握手后的业务消息放行。
    assert!(enforce_handshake_order("completed", &mut ready_seen).is_ok());
}

#[test]
fn affected_file_paths_are_confined_to_workspace() {
    let workspace = Path::new("/tmp/workspace");
    // 相对路径与绝对路径都规整为工作区相对形式。
    assert_eq!(
        normalize_workspace_file(workspace, "src/a.rs").as_deref(),
        Some("src/a.rs")
    );
    assert_eq!(
        normalize_workspace_file(workspace, "/tmp/workspace/src/a.rs").as_deref(),
        Some("src/a.rs")
    );
    // 内部 .. 可消解时允许。
    assert_eq!(
        normalize_workspace_file(workspace, "src/../lib/b.rs").as_deref(),
        Some("lib/b.rs")
    );
    // 越界路径（../ 逃逸、工作区外绝对路径）一律拒绝。
    assert_eq!(normalize_workspace_file(workspace, "../outside.rs"), None);
    assert_eq!(
        normalize_workspace_file(workspace, "src/../../escape.rs"),
        None
    );
    assert_eq!(normalize_workspace_file(workspace, "/etc/passwd"), None);
    assert_eq!(normalize_workspace_file(workspace, ""), None);
}
