//! Tauri 命令层——前端通过 invoke 调用。
//!
//! 所有命令遵守 AGENTS.md：HTTP/进程 I/O 均在 tokio 运行时内异步执行，
//! 不阻塞 Tauri 主线程；Mutex 临界区内只做内存读写。

use serde_json::Value;
use tauri::{AppHandle, State};

use super::config::{QdrantConfig, RagConfigStore, RagKbConfig};
use super::logs::{RagLogEntry, RagLogStore};
use super::manager::RagManager;
use super::transport::{err_to_string, no_port_error};

/// 启动 sidecar（若未启动）并返回运行状态。
#[tauri::command]
pub async fn rag_start(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
) -> Result<Value, String> {
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .map_err(err_to_string)?;
    Ok(serde_json::json!({
        "running": true,
        "port": handle.port,
    }))
}

/// 停止 sidecar。
#[tauri::command]
pub fn rag_stop(manager: State<'_, RagManager>) -> Result<(), String> {
    manager.stop();
    Ok(())
}

/// 查询 sidecar 状态（是否运行、端口）。
#[tauri::command]
pub fn rag_status(manager: State<'_, RagManager>) -> Result<Value, String> {
    Ok(serde_json::json!({
        "running": manager.is_running(),
        "port": manager.current().map(|h| h.port),
    }))
}

/// 读取当前知识库配置（不脱敏，调用方需自行注意 UI 展示）。
#[tauri::command]
pub fn rag_get_kb_config(config_store: State<'_, RagConfigStore>) -> Result<RagKbConfig, String> {
    config_store.get_or_load().map_err(err_to_string)
}

/// 保存知识库配置：落盘 + 更新内存 + 若 sidecar 在运行则推送 reload。
#[tauri::command]
pub async fn rag_save_kb_config(
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> Result<Value, String> {
    // 1. 落盘（锁外 I/O）
    config.save().map_err(err_to_string)?;
    // 2. 更新内存快照
    config_store.replace(config.clone());
    // 3. 若 sidecar 在运行，热推送
    if let Some(handle) = manager.current() {
        handle
            .transport
            .reload_config(&config)
            .await
            .map_err(err_to_string)?;
    }
    Ok(serde_json::json!({ "saved": true, "reloaded": manager.is_running() }))
}

/// 代理调用 sidecar 的 GET /health（便于前端确认服务就绪）。
#[tauri::command]
pub async fn rag_health(manager: State<'_, RagManager>) -> Result<Value, String> {
    let handle = manager
        .current()
        .ok_or_else(|| err_to_string(no_port_error()))?;
    handle.transport.health().await.map_err(err_to_string)
}

/// 直接测试 Qdrant HTTP 端点，避免把“向量库连接测试”误判为 sidecar 健康检查。
#[tauri::command]
pub async fn rag_test_qdrant(config: QdrantConfig) -> Result<Value, String> {
    let url = qdrant_health_url(&config)?;
    let timeout_secs = if config.timeout.is_finite() && config.timeout > 0.0 {
        config.timeout
    } else {
        return Err("Qdrant 超时必须是大于 0 的数字".to_string());
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs_f64(timeout_secs))
        .build()
        .map_err(|error| format!("构造 Qdrant HTTP client 失败：{error}"))?;

    let mut request = client.get(url.clone());
    if !config.api_key.trim().is_empty() {
        request = request.header("api-key", config.api_key.trim());
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("GET {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET {url} 返回非 2xx 状态：{status}"));
    }

    Ok(serde_json::json!({
        "ok": true,
        "status": status.as_u16(),
    }))
}

/// 代理调用 sidecar 的 GET /config（脱敏视图）。
#[tauri::command]
pub async fn rag_sidecar_config(manager: State<'_, RagManager>) -> Result<Value, String> {
    let handle = manager
        .current()
        .ok_or_else(|| err_to_string(no_port_error()))?;
    handle.transport.get_config().await.map_err(err_to_string)
}

/// 返回当前内存中的 RAG sidecar 滚动日志。
#[tauri::command]
pub fn rag_logs_snapshot(log_store: State<'_, RagLogStore>) -> Result<Vec<RagLogEntry>, String> {
    Ok(log_store.snapshot())
}

/// 清空当前内存日志窗口。
#[tauri::command]
pub fn rag_logs_clear(log_store: State<'_, RagLogStore>) -> Result<(), String> {
    log_store.clear();
    Ok(())
}

fn qdrant_health_url(config: &QdrantConfig) -> Result<reqwest::Url, String> {
    let base = config.url.trim();
    if base.is_empty() {
        return Err("Qdrant HTTP 端点不能为空".to_string());
    }

    let url = reqwest::Url::parse(&format!("{}/healthz", base.trim_end_matches('/')))
        .map_err(|error| format!("Qdrant HTTP 端点无效：{error}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        scheme => Err(format!(
            "Qdrant HTTP 端点必须使用 http/https，当前为 {scheme}"
        )),
    }
}
