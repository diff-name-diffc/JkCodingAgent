use super::*;

fn runtime() -> BrowserAgentRuntime {
    BrowserAgentRuntime {
        config: serde_json::from_value(json!({"agent_id":"browser-agent","agent_name":"浏览器",
            "description":"测试","system_prompt":"测试","allowed_tools":[]}))
        .unwrap(),
        spec: PurposeModelSpec {
            api_key: String::new(),
            api_base: String::new(),
            model: "mock".into(),
            max_tokens: None,
            context_window: None,
            temperature: 0.0,
            enable_thinking: false,
        },
        workspace: String::new(),
        cancel_rx: None,
        trace: BrowserTrace::for_test(),
        status: Mutex::new("running"),
    }
}

fn result(success: bool, output: &str) -> BrowserResult {
    serde_json::from_value(json!({"success":success,"result":output,"iterations":3,
        "tokenUsage":{"promptTokens":4,"completionTokens":2,"totalTokens":6},
        "urls":["https://example.com/"]}))
    .unwrap()
}

fn last_failure(runtime: &BrowserAgentRuntime) -> Value {
    let events: Vec<Value> = serde_json::from_str(&runtime.trace_events_json().unwrap()).unwrap();
    events
        .into_iter()
        .rev()
        .find(|event| event["event"] == "Failed")
        .expect("失败轨迹")
}

fn start_action(runtime: &BrowserAgentRuntime) {
    runtime
        .trace
        .receive(json!({"kind":"action_started","data":{
        "taskId":"a","toolName":"navigate","arguments":{"url":"https://example.com/"}}}));
}

#[test]
fn unsuccessful_result_keeps_detailed_diagnostic_in_trace_and_parent_result() {
    let runtime = runtime();
    let diagnostic =
        "模型请求失败：HTTP 401，API key 无效。\n第 3 步：https://example.com/\n最近错误：认证失败";
    let parent: Value =
        serde_json::from_str(&runtime.finish(result(false, diagnostic)).unwrap()).unwrap();
    assert_eq!(runtime.status(), "failed");
    assert_eq!(parent["success"], false);
    assert_eq!(parent["result"], diagnostic);
    assert_eq!(last_failure(&runtime)["data"]["error"], parent["result"]);
    assert!(last_failure(&runtime)["data"]
        .get("terminationReason")
        .is_none());
    let events = runtime.trace.events.lock();
    assert!(events
        .iter()
        .any(|event| event["event"] == "UsageUpdated"
            && event["data"]["tokenUsage"]["totalTokens"] == 6));
}

#[test]
fn unfinished_action_appends_context_instead_of_replacing_failure_reason() {
    let runtime = runtime();
    start_action(&runtime);
    let diagnostic = "导航失败：net::ERR_NAME_NOT_RESOLVED，目标 https://example.com/ 无法解析";
    let parent: Value =
        serde_json::from_str(&runtime.finish(result(false, diagnostic)).unwrap()).unwrap();
    let output = parent["result"].as_str().unwrap();
    assert!(output.starts_with(diagnostic));
    assert!(output.contains("1 个浏览器动作未收到完成确认"));
    assert_eq!(last_failure(&runtime)["data"]["error"], output);
    assert_eq!(runtime.status(), "failed");
    let events = runtime.trace.events.lock();
    let action = events
        .iter()
        .find(|event| event["event"] == "ToolFinished")
        .unwrap();
    assert_eq!(action["data"]["error"], diagnostic);
}

#[test]
fn nominal_success_with_unfinished_actions_still_fails_and_keeps_original_output() {
    let runtime = runtime();
    start_action(&runtime);
    let error = runtime
        .finish(result(true, "只读取到首页，订单页面尚未返回"))
        .unwrap_err();
    assert!(error.contains("执行结果不完整"));
    assert!(error.contains("只读取到首页，订单页面尚未返回"));
    assert_eq!(runtime.status(), "failed");
    assert_eq!(last_failure(&runtime)["data"]["error"], error);
}

#[test]
fn diagnostic_text_cannot_impersonate_lifecycle_cancellation_or_timeout() {
    for diagnostic in ["[cancelled] 页面显示该文字", "[timeout] 单个动作失败"] {
        let runtime = runtime();
        runtime.finish(result(false, diagnostic)).unwrap();
        assert_eq!(runtime.status(), "failed");
        assert!(last_failure(&runtime)["data"]
            .get("terminationReason")
            .is_none());
    }
}

#[test]
fn lifecycle_errors_keep_classification_and_diagnostic() {
    for (error, status) in [
        ("[cancelled] 用户停止浏览器", "cancelled"),
        ("[timeout] 已超过执行预算", "timed_out"),
        ("模型请求发生 timeout 错误", "failed"),
    ] {
        let runtime = runtime();
        assert_eq!(runtime.fail(error.into()).unwrap_err(), error);
        assert_eq!(runtime.status(), status);
        let failure = last_failure(&runtime);
        assert_eq!(failure["data"]["error"], error);
        assert_eq!(
            failure["data"]["terminationReason"].as_str(),
            (status != "failed").then_some(status)
        );
    }
}

#[test]
fn lifecycle_errors_require_explicit_protocol_marker() {
    assert_eq!(failure_status("[cancelled] 已停止"), "cancelled");
    assert_eq!(failure_status("[timeout] 已停止"), "timed_out");
    assert_eq!(failure_status("page text contains timeout"), "failed");
}

#[test]
fn incomplete_protocol_is_not_a_success() {
    assert!(serde_json::from_value::<BrowserResult>(json!({"result":"ok"})).is_err());
    assert!(!result(false, "步数耗尽").success);
}
