//! RAG 知识库配置——权威存储位于 Rust 宿主侧。
//!
//! 设计约定（见 AGENTS.md 与 rag/README.md）：
//! - 配置文件：`~/.jkcodingagent/rag/config.json`
//! - 本模块是配置的唯一写入方；Python sidecar 只接收、不回写
//! - 启动 sidecar 时通过环境变量注入；变更时通过 HTTP /config/reload 推送
//!
//! 骨架阶段只提供结构体 + load/save；真实业务字段可在后续迭代扩展，
//! 但新增字段必须同步更新 `rag/src/rag_server/config.py` 的对应 Pydantic 模型，
//! 否则 Python 侧 reload 会因 schema 不匹配而失败。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Qdrant 连接配置（外部独立部署的向量库实例）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantConfig {
    /// Qdrant HTTP 端点，例如 `http://127.0.0.1:6333`。
    #[serde(default = "default_qdrant_url")]
    pub url: String,
    /// Qdrant API Key，可空。
    #[serde(default)]
    pub api_key: String,
    /// collection 命名前缀，用于多项目/多租户隔离。
    #[serde(default = "default_collection_prefix")]
    pub collection_prefix: String,
    /// 请求超时（秒）。
    #[serde(default = "default_timeout")]
    pub timeout: f64,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            url: default_qdrant_url(),
            api_key: String::new(),
            collection_prefix: default_collection_prefix(),
            timeout: default_timeout(),
        }
    }
}

fn default_qdrant_url() -> String {
    "http://127.0.0.1:6333".to_string()
}
fn default_collection_prefix() -> String {
    "jk_".to_string()
}
fn default_timeout() -> f64 {
    10.0
}

/// Embedding 模型配置（走 OpenAI 兼容 API，复用宿主已有 LLM 配置）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    /// OpenAI 兼容的 embedding 接口地址。
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_embedding_model")]
    pub model: String,
    #[serde(default = "default_dimension")]
    pub dimension: u32,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            base_url: String::new(),
            api_key: String::new(),
            model: default_embedding_model(),
            dimension: default_dimension(),
        }
    }
}

fn default_provider() -> String {
    "openai_compatible".to_string()
}
fn default_embedding_model() -> String {
    "text-embedding-3-small".to_string()
}
fn default_dimension() -> u32 {
    1536
}

/// RAG 知识库的完整运行时配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagKbConfig {
    #[serde(default)]
    pub qdrant: QdrantConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "INFO".to_string()
}

impl RagKbConfig {
    /// 配置文件路径：`~/.jkcodingagent/rag/config.json`。
    pub fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("failed to resolve home directory")?;
        Ok(home.join(".jkcodingagent").join("rag").join("config.json"))
    }

    /// 从磁盘加载；文件不存在时返回默认值并尝试落盘一份默认配置。
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        match fs::read_to_string(&path) {
            Ok(content) => {
                let config: Self = serde_json::from_str(&content)
                    .with_context(|| format!("parse {}", path.display()))?;
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let config = Self::default();
                // 落盘默认配置，便于用户了解可用字段
                let _ = Self::save_raw(&config);
                Ok(config)
            }
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    /// 写入磁盘。
    pub fn save(&self) -> Result<()> {
        Self::save_raw(self)
    }

    fn save_raw(config: &Self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(config).context("serialize rag config")?;
        fs::write(&path, body).with_context(|| format!("write {}", path.display()))
    }

    /// 将配置展开为注入 sidecar 子进程的环境变量键值列表。
    ///
    /// 键名与 `rag/src/rag_server/config.py` 的 `RagSettings.from_env()` 严格对应，
    /// 修改任一侧必须同步另一侧。
    pub fn to_env_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("RAG_QDRANT_URL", self.qdrant.url.clone()),
            ("RAG_QDRANT_API_KEY", self.qdrant.api_key.clone()),
            (
                "RAG_QDRANT_COLLECTION_PREFIX",
                self.qdrant.collection_prefix.clone(),
            ),
            ("RAG_QDRANT_TIMEOUT", self.qdrant.timeout.to_string()),
            ("RAG_EMBEDDING_PROVIDER", self.embedding.provider.clone()),
            ("RAG_EMBEDDING_BASE_URL", self.embedding.base_url.clone()),
            ("RAG_EMBEDDING_API_KEY", self.embedding.api_key.clone()),
            ("RAG_EMBEDDING_MODEL", self.embedding.model.clone()),
            (
                "RAG_EMBEDDING_DIMENSION",
                self.embedding.dimension.to_string(),
            ),
            ("RAG_LOG_LEVEL", self.log_level.clone()),
        ]
    }
}

/// 进程级配置持有者：sidecar 启动前读取，reload 时更新内存并通知 sidecar。
///
/// 用 Mutex 保护以便 Tauri State 共享；临界区内只做内存读写，不做 I/O
/// （save 与 HTTP reload 由调用方在锁外完成，符合 AGENTS.md 持锁禁 I/O 规则）。
#[derive(Default)]
pub struct RagConfigStore {
    inner: Mutex<Option<RagKbConfig>>,
}

impl RagConfigStore {
    /// 取一份当前配置的快照；尚未加载则从磁盘读取并缓存。
    pub fn get_or_load(&self) -> Result<RagKbConfig> {
        if let Some(snapshot) = self.inner.lock().as_ref() {
            return Ok(snapshot.clone());
        }
        let loaded = RagKbConfig::load()?;
        *self.inner.lock() = Some(loaded.clone());
        Ok(loaded)
    }

    /// 用一份新配置替换内存快照（不落盘、不通知 sidecar，由调用方组合）。
    pub fn replace(&self, config: RagKbConfig) {
        *self.inner.lock() = Some(config);
    }
}
