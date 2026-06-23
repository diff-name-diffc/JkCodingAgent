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

    /// `GET /config`（sidecar 返回脱敏配置）。
    pub async fn get_config(&self) -> Result<Value> {
        self.get_json("/config").await
    }

    /// `POST /config/reload`——把新配置推送到 sidecar 内存。
    ///
    /// body 结构与 `rag/src/rag_server/routers/config.py::ReloadPayload` 对齐。
    pub async fn reload_config(&self, config: &RagKbConfig) -> Result<Value> {
        let payload = ReloadPayload::from_config(config);
        self.post_json("/config/reload", &payload).await
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} non-2xx"))?;
        resp.json::<Value>()
            .await
            .with_context(|| format!("decode GET {url} body"))
    }

    async fn post_json<B: Serialize>(&self, path: &str, body: &B) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .with_context(|| format!("POST {url} non-2xx"))?;
        resp.json::<Value>()
            .await
            .with_context(|| format!("decode POST {url} body"))
    }
}

/// `/config/reload` 请求体。字段名与 Python 侧 ReloadPayload 一致。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReloadPayload {
    qdrant: QdrantPayload,
    embedding: EmbeddingPayload,
    log_level: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QdrantPayload {
    url: String,
    api_key: String,
    collection_prefix: String,
    timeout: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingPayload {
    provider: String,
    base_url: String,
    api_key: String,
    model: String,
    dimension: u32,
}

impl ReloadPayload {
    fn from_config(c: &RagKbConfig) -> Self {
        Self {
            qdrant: QdrantPayload {
                url: c.qdrant.url.clone(),
                api_key: c.qdrant.api_key.clone(),
                collection_prefix: c.qdrant.collection_prefix.clone(),
                timeout: c.qdrant.timeout,
            },
            embedding: EmbeddingPayload {
                provider: c.embedding.provider.clone(),
                base_url: c.embedding.base_url.clone(),
                api_key: c.embedding.api_key.clone(),
                model: c.embedding.model.clone(),
                dimension: c.embedding.dimension,
            },
            log_level: c.log_level.clone(),
        }
    }
}

/// 把 anyhow 错误转为前端友好的字符串（commands 层统一用 String 错误）。
pub fn err_to_string(error: anyhow::Error) -> String {
    format!("{error:#}")
}

/// 把 "未知端口" 的语义错误统一封装。
pub fn no_port_error() -> anyhow::Error {
    anyhow!("sidecar 尚未完成端口握手，无法发起 HTTP 调用")
}
