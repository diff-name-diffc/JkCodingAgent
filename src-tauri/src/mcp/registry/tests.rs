//! registry 纯函数单测：配置合并、新鲜度、解析、状态聚合与工具名规范化。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::model::Tool;

use super::check::aggregate_server_statuses;
use super::config::{is_fresh, merge_configs};
use super::resolve::{
    normalize_transport_name, resolve_cwd, resolve_mcp_tool, resolve_server_config,
    resolve_transport_kind, sanitize_tool_name,
};
use crate::mcp::{McpAggregateStatus, McpConfig, McpServerConfig, McpServerState, McpServerStatus};

fn server(command: &str) -> McpServerConfig {
    McpServerConfig {
        command: Some(command.to_string()),
        ..Default::default()
    }
}

#[test]
fn merge_configs_prefers_project_entries_on_name_collision() {
    let mut global = McpConfig::default();
    global
        .servers
        .insert("shared".to_string(), server("global-bin"));
    global
        .servers
        .insert("global-only".to_string(), server("g"));
    let mut project = McpConfig::default();
    project
        .servers
        .insert("shared".to_string(), server("project-bin"));
    project
        .servers
        .insert("project-only".to_string(), server("p"));

    let merged = merge_configs(global, project);
    assert_eq!(merged.servers.len(), 3);
    assert_eq!(
        merged.servers["shared"].command.as_deref(),
        Some("project-bin")
    );
    assert!(merged.servers.contains_key("global-only"));
    assert!(merged.servers.contains_key("project-only"));
}

#[test]
fn merge_configs_handles_empty_sides() {
    let empty = McpConfig::default();
    let mut one = McpConfig::default();
    one.servers.insert("a".to_string(), server("a"));

    assert!(merge_configs(empty.clone(), McpConfig::default())
        .servers
        .is_empty());
    assert_eq!(merge_configs(empty.clone(), one.clone()).servers.len(), 1);
    assert_eq!(merge_configs(one, empty).servers.len(), 1);
}

#[test]
fn is_fresh_boundary_is_inclusive_at_max_age() {
    let max_age = Duration::from_secs(300);
    let now = 1_000_000_000_000;
    assert!(is_fresh(now - 300_000, now, max_age));
    assert!(!is_fresh(now - 300_001, now, max_age));
    assert!(is_fresh(now, now, max_age));
}

#[test]
fn resolve_cwd_requires_absolute_paths_in_global_scope() {
    let base = PathBuf::from("/tmp/project");
    assert_eq!(
        resolve_cwd(Some(&base), "s", "sub/dir").unwrap(),
        base.join("sub/dir")
    );
    assert_eq!(
        resolve_cwd(Some(&base), "s", "/abs/path").unwrap(),
        PathBuf::from("/abs/path")
    );
    assert_eq!(
        resolve_cwd(None, "s", "/abs/path").unwrap(),
        PathBuf::from("/abs/path")
    );
    let error = resolve_cwd(None, "fetch", "relative").unwrap_err();
    assert!(error.contains("fetch"));
    assert!(error.contains("绝对路径"));
}

#[test]
fn resolve_server_config_rejects_invalid_shapes() {
    let base = Path::new("/tmp/project");
    let missing_command = McpServerConfig::default();
    assert!(resolve_server_config(Some(base), "a", missing_command).is_err());

    let zero_timeout = McpServerConfig {
        command: Some("node".to_string()),
        startup_timeout_seconds: Some(0),
        ..Default::default()
    };
    let error = resolve_server_config(Some(base), "a", zero_timeout).unwrap_err();
    assert!(error.contains("startupTimeoutSeconds"));

    let unknown_transport = McpServerConfig {
        transport: Some("carrier-pigeon".to_string()),
        command: Some("node".to_string()),
        ..Default::default()
    };
    assert!(resolve_server_config(Some(base), "a", unknown_transport).is_err());
}

#[test]
fn transport_kind_inference_and_aliases() {
    assert_eq!(resolve_transport_kind(&server("node")).unwrap(), "stdio");
    assert_eq!(
        resolve_transport_kind(&McpServerConfig {
            url: Some("http://x".to_string()),
            ..Default::default()
        })
        .unwrap(),
        "streamable_http"
    );
    assert_eq!(
        normalize_transport_name("Streamable-HTTP"),
        "streamable_http"
    );
    assert_eq!(normalize_transport_name("http"), "streamable_http");
    assert_eq!(normalize_transport_name("stdio"), "stdio");
}

#[test]
fn aggregate_statuses_count_enabled_and_healthy() {
    let healthy = McpServerStatus {
        name: "a".to_string(),
        transport: "stdio".to_string(),
        enabled: true,
        state: McpServerState::Healthy,
        summary: String::new(),
        error: None,
        tool_count: 0,
        tools: vec![],
    };
    let disabled = McpServerStatus {
        enabled: false,
        name: "b".to_string(),
        ..healthy.clone()
    };
    let failed = McpServerStatus {
        name: "c".to_string(),
        state: McpServerState::ConnectionFailed,
        ..healthy.clone()
    };

    let (aggregate, _, _) = aggregate_server_statuses(&[]);
    assert!(matches!(aggregate, McpAggregateStatus::NotConfigured));
    let (aggregate, enabled, healthy_count) =
        aggregate_server_statuses(&[healthy.clone(), disabled]);
    assert!(matches!(aggregate, McpAggregateStatus::Healthy));
    assert_eq!((enabled, healthy_count), (1, 1));
    let (aggregate, enabled, healthy_count) = aggregate_server_statuses(&[healthy, failed]);
    assert!(matches!(aggregate, McpAggregateStatus::Degraded));
    assert_eq!((enabled, healthy_count), (2, 1));
}

#[test]
fn tool_names_are_sanitized_and_deduplicated() {
    assert_eq!(sanitize_tool_name("My-Server.1"), "my_server_1");
    assert_eq!(sanitize_tool_name("___"), "tool");

    let tool = |name: &str| {
        serde_json::from_value::<Tool>(serde_json::json!({
            "name": name,
            "inputSchema": { "type": "object" }
        }))
        .unwrap()
    };
    let mut used = HashMap::new();
    let first = resolve_mcp_tool("srv", tool("read"), &mut used);
    let second = resolve_mcp_tool("srv", tool("read"), &mut used);
    assert_eq!(first.canonical_name, "mcp__srv__read");
    assert_eq!(second.canonical_name, "mcp__srv__read__2");
}
