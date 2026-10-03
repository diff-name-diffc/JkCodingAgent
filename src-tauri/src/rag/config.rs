//! RAG 知识库配置——权威存储位于 Rust 宿主侧。
//!
//! 设计约定（见 AGENTS.md 与 rag/README.md）：
//! - 配置存全局库 app_config 表（键 `rag`）；早期开发版的文件存储已退出当前基线
//! - 本模块是配置的唯一写入方；Python sidecar 只接收、不回写
//! - 启动 sidecar 时通过环境变量注入；变更时通过 HTTP /config/reload 推送
//!
//! 骨架阶段只提供结构体 + load/save；真实业务字段可在后续迭代扩展，
//! 但新增字段必须同步更新 `rag/src/rag_server/config.py` 的对应 Pydantic 模型，
//! 否则 Python 侧 reload 会因 schema 不匹配而失败。

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Qdrant 连接配置（外部独立部署的向量库实例）。
///
/// 默认值唯一事实源是手写 `Default` impl；结构级 `#[serde(default)]` 让
/// 反序列化缺失字段时直接取自 `Self::default()`（不再逐字段声明 default fn）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QdrantConfig {
    /// Qdrant HTTP 端点，例如 `http://127.0.0.1:6333`。
    pub url: String,
    /// Qdrant API Key，可空。
    pub api_key: String,
    /// collection 命名前缀，用于多项目/多租户隔离。
    pub collection_prefix: String,
    /// 请求超时（秒）。
    pub timeout: f64,
    /// Qdrant 命名稠密向量。
    pub dense_vector_name: String,
    /// Qdrant 命名稀疏向量。
    pub sparse_vector_name: String,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:6333".to_string(),
            api_key: String::new(),
            collection_prefix: "jk_".to_string(),
            timeout: 10.0,
            dense_vector_name: "dense".to_string(),
            sparse_vector_name: "sparse".to_string(),
        }
    }
}

/// Embedding 模型配置（走 OpenAI 兼容 API，复用宿主已有 LLM 配置）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmbeddingConfig {
    pub provider: String,
    /// OpenAI 兼容的 embedding 接口地址。
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimension: u32,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "openai_compatible".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            model: "text-embedding-3-small".to_string(),
            dimension: 1536,
        }
    }
}

/// 稀疏向量模型配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SparseEmbeddingConfig {
    pub provider: String,
    pub model: String,
}

impl Default for SparseEmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "fastembed".to_string(),
            model: "Qdrant/bm25".to_string(),
        }
    }
}

/// 父子分片配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChunkingConfig {
    pub parent_chunk_size: u32,
    pub parent_chunk_overlap: u32,
    pub child_chunk_size: u32,
    pub child_chunk_overlap: u32,
    pub separators: Vec<String>,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            parent_chunk_size: 2000,
            parent_chunk_overlap: 200,
            child_chunk_size: 400,
            child_chunk_overlap: 80,
            separators: ["\n\n", "\n", "。", "；", ". ", " ", ""]
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }
}

/// OCR 配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OcrConfig {
    pub enabled: bool,
    pub use_cuda: bool,
    pub pdf_image_width_ratio: f64,
    pub pdf_image_height_ratio: f64,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            use_cuda: false,
            pdf_image_width_ratio: 0.6,
            pdf_image_height_ratio: 0.6,
        }
    }
}

/// RAG 知识库的完整运行时配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RagKbConfig {
    pub qdrant: QdrantConfig,
    pub embedding: EmbeddingConfig,
    pub sparse_embedding: SparseEmbeddingConfig,
    pub chunking: ChunkingConfig,
    pub ocr: OcrConfig,
    pub log_level: String,
}

impl Default for RagKbConfig {
    fn default() -> Self {
        Self {
            qdrant: QdrantConfig::default(),
            embedding: EmbeddingConfig::default(),
            sparse_embedding: SparseEmbeddingConfig::default(),
            chunking: ChunkingConfig::default(),
            ocr: OcrConfig::default(),
            log_level: "INFO".to_string(),
        }
    }
}

impl RagKbConfig {
    /// 从全局库加载（app_config 表 `rag` 键）；未配置时返回默认值。
    pub fn load_from_db(db: &crate::agent::db::DispatcherDb) -> Result<Self> {
        match db.get_app_config_json(crate::agent::db::app_config::RAG_KEY) {
            Ok(Some(raw)) => serde_json::from_str(&raw).context("parse rag config from app_config"),
            Ok(None) => Ok(Self::default()),
            Err(error) => Err(error.context("load rag config from app_config")),
        }
    }

    /// 写入全局库（app_config 表 `rag` 键）。
    pub fn save_to_db(&self, db: &crate::agent::db::DispatcherDb) -> Result<()> {
        let body = serde_json::to_string(self).context("serialize rag config")?;
        db.set_app_config_json(crate::agent::db::app_config::RAG_KEY, &body)
            .context("save rag config to app_config")
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
            (
                "RAG_QDRANT_DENSE_VECTOR_NAME",
                self.qdrant.dense_vector_name.clone(),
            ),
            (
                "RAG_QDRANT_SPARSE_VECTOR_NAME",
                self.qdrant.sparse_vector_name.clone(),
            ),
            ("RAG_EMBEDDING_PROVIDER", self.embedding.provider.clone()),
            ("RAG_EMBEDDING_BASE_URL", self.embedding.base_url.clone()),
            ("RAG_EMBEDDING_API_KEY", self.embedding.api_key.clone()),
            ("RAG_EMBEDDING_MODEL", self.embedding.model.clone()),
            (
                "RAG_EMBEDDING_DIMENSION",
                self.embedding.dimension.to_string(),
            ),
            (
                "RAG_SPARSE_EMBEDDING_PROVIDER",
                self.sparse_embedding.provider.clone(),
            ),
            (
                "RAG_SPARSE_EMBEDDING_MODEL",
                self.sparse_embedding.model.clone(),
            ),
            (
                "RAG_PARENT_CHUNK_SIZE",
                self.chunking.parent_chunk_size.to_string(),
            ),
            (
                "RAG_PARENT_CHUNK_OVERLAP",
                self.chunking.parent_chunk_overlap.to_string(),
            ),
            (
                "RAG_CHILD_CHUNK_SIZE",
                self.chunking.child_chunk_size.to_string(),
            ),
            (
                "RAG_CHILD_CHUNK_OVERLAP",
                self.chunking.child_chunk_overlap.to_string(),
            ),
            ("RAG_OCR_ENABLED", self.ocr.enabled.to_string()),
            ("RAG_OCR_USE_CUDA", self.ocr.use_cuda.to_string()),
            (
                "RAG_OCR_PDF_IMAGE_WIDTH_RATIO",
                self.ocr.pdf_image_width_ratio.to_string(),
            ),
            (
                "RAG_OCR_PDF_IMAGE_HEIGHT_RATIO",
                self.ocr.pdf_image_height_ratio.to_string(),
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
    /// 配置权威源的读取入口。store 在 DispatcherState 之前注册（builder 链），
    /// DB 打开后由 setup 注入。
    db: Mutex<Option<crate::agent::db::DispatcherDb>>,
}

impl RagConfigStore {
    /// 注入全局 DB（app 启动 setup 中调用一次）。
    pub fn attach_db(&self, db: crate::agent::db::DispatcherDb) {
        *self.db.lock() = Some(db);
    }

    /// 取一份当前配置的快照；尚未加载则从全局库读取并缓存。
    pub fn get_or_load(&self) -> Result<RagKbConfig> {
        if let Some(snapshot) = self.inner.lock().as_ref() {
            return Ok(snapshot.clone());
        }
        let db = self
            .db
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("RAG 配置存储尚未挂载数据库（应用初始化未完成）"))?;
        let loaded = RagKbConfig::load_from_db(&db)?;
        *self.inner.lock() = Some(loaded.clone());
        Ok(loaded)
    }

    /// 用一份新配置替换内存快照（不落库、不通知 sidecar，由调用方组合）。
    pub fn replace(&self, config: RagKbConfig) {
        *self.inner.lock() = Some(config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults_from_struct_default() {
        let config: RagKbConfig =
            serde_json::from_str(r#"{"qdrant":{"url":"http://qdrant:6333"}}"#).unwrap();
        assert_eq!(config.qdrant.url, "http://qdrant:6333");
        assert_eq!(config.qdrant.collection_prefix, "jk_");
        assert_eq!(config.qdrant.timeout, 10.0);
        assert_eq!(config.embedding.model, "text-embedding-3-small");
        assert_eq!(config.chunking.parent_chunk_size, 2000);
        assert!(config.ocr.enabled);
        assert_eq!(config.log_level, "INFO");
    }

    #[test]
    fn partial_json_within_subconfig_keeps_sibling_defaults() {
        let config: RagKbConfig =
            serde_json::from_str(r#"{"chunking":{"childChunkSize":128},"logLevel":"DEBUG"}"#)
                .unwrap();
        assert_eq!(config.chunking.child_chunk_size, 128);
        assert_eq!(config.chunking.child_chunk_overlap, 80);
        assert_eq!(config.log_level, "DEBUG");
        assert_eq!(config.sparse_embedding.provider, "fastembed");
    }

    #[test]
    fn serialize_emits_all_fields_and_roundtrips() {
        let config = RagKbConfig::default();
        let body = serde_json::to_string(&config).unwrap();
        for field in [
            "\"url\"",
            "\"collectionPrefix\"",
            "\"dimension\"",
            "\"parentChunkSize\"",
            "\"pdfImageWidthRatio\"",
            "\"logLevel\"",
        ] {
            assert!(body.contains(field), "缺少字段 {field}：{body}");
        }
        let parsed: RagKbConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.chunking.separators, config.chunking.separators);
        assert_eq!(parsed.qdrant.timeout, config.qdrant.timeout);
    }

    #[test]
    fn empty_object_yields_full_defaults() {
        let config: RagKbConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, RagKbConfig::default());
    }
}
