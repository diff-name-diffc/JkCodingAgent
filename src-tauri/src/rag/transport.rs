//! Sidecar HTTP 传输层——通过 reqwest 调用已启动的 rag-server。
//!
//! 端口来源：`RagHandle::port`，由 manager 在握手阶段解析 stdout 得到。
//! 所有方法均为 async，调用方（commands.rs）须在 tokio 运行时中执行。

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

use super::config::RagKbConfig;

/// 与 sidecar 通信的 HTTP 客户端封装。
#[derive(Clone)]
pub struct RagTransport {
    client: Client,
    base_url: String,
}

impl RagTransport {
    pub fn new(port: u16) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            client,
            base_url: format!("http://127.0.0.1:{port}"),
        })
    }

    /// `GET /health`。
    pub async fn health(&self) -> Result<Value> {
        self.get_json("/health").await
    }

    /// `POST /config/reload`——把新配置推送到 sidecar 内存。
    ///
    /// body 直接序列化 `RagKbConfig`（camelCase），形状与
    /// `rag/src/rag_server/routers/config.py::ReloadPayload` 字段一一对应；
    /// Python 侧 pydantic 默认忽略未知字段，配置新增字段不会破坏解析。
    pub async fn reload_config(&self, config: &RagKbConfig) -> Result<Value> {
        self.post_json("/config/reload", config).await
    }

    /// `POST /test/qdrant`——由无状态 sidecar 使用本次请求中的配置测试 Qdrant。
    pub async fn test_qdrant(&self, config: &RagKbConfig) -> Result<Value> {
        self.post_json("/test/qdrant", config).await
    }

    /// `POST /test/embedding`——由无状态 sidecar 使用本次请求中的配置测试 Embedding。
    pub async fn test_embedding(&self, config: &RagKbConfig) -> Result<Value> {
        self.post_json("/test/embedding", config).await
    }

    /// `POST /ingest/jobs`——启动导入任务。
    pub async fn start_ingest_job(
        &self,
        project_id: &str,
        project_path: &str,
        files: &[String],
    ) -> Result<Value> {
        let payload = IngestPayload {
            project_id: project_id.to_string(),
            project_path: project_path.to_string(),
            files: files.to_vec(),
            options: IngestOptionsPayload {
                replace_existing: true,
            },
        };
        self.post_json("/ingest/jobs", &payload).await
    }

    /// `GET /ingest/jobs/{job_id}`——查询导入任务状态。
    pub async fn ingest_job_status(&self, job_id: &str) -> Result<Value> {
        self.get_json(&format!("/ingest/jobs/{job_id}")).await
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        decode_json_response("GET", &url, resp).await
    }

    async fn post_json<B: Serialize>(&self, path: &str, body: &B) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode_json_response("POST", &url, resp).await
    }
}

async fn decode_json_response(method: &str, url: &str, resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body = resp
        .text()
        .await
        .with_context(|| format!("read {method} {url} body"))?;
    if !status.is_success() {
        return Err(anyhow!("{method} {url} 返回 HTTP {status}: {body}"));
    }
    serde_json::from_str::<Value>(&body).with_context(|| format!("decode {method} {url} body"))
}

/// `/ingest/jobs` 请求体。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestPayload {
    project_id: String,
    project_path: String,
    files: Vec<String>,
    options: IngestOptionsPayload,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestOptionsPayload {
    replace_existing: bool,
}

/// 把 "未知端口" 的语义错误统一封装。
pub fn no_port_error() -> anyhow::Error {
    anyhow!("sidecar 尚未完成端口握手，无法发起 HTTP 调用")
}
