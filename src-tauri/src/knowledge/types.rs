use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCollection {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeModelConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSettings {
    #[serde(default)]
    pub text_model: KnowledgeModelConfig,
    #[serde(default)]
    pub vision_model: KnowledgeModelConfig,
    #[serde(default)]
    pub embedding_model: KnowledgeModelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIngestJob {
    pub id: String,
    pub collection_id: String,
    pub source_name: String,
    pub source_path: String,
    pub status: String,
    pub message: String,
    pub pages_written: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePageSummary {
    pub collection_id: String,
    pub path: String,
    pub relative_path: String,
    pub title: String,
    pub page_type: String,
    pub tags: Vec<String>,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePageContent {
    pub collection_id: String,
    pub path: String,
    pub relative_path: String,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchResult {
    pub collection_id: String,
    pub collection_name: String,
    pub path: String,
    pub relative_path: String,
    pub title: String,
    pub page_type: String,
    pub snippet: String,
    pub score: f32,
    pub vector_score: f32,
    pub token_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeVectorStats {
    pub collection_id: String,
    pub page_count: usize,
    pub chunk_count: usize,
    pub dimension: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraph {
    pub nodes: Vec<KnowledgeGraphNode>,
    pub edges: Vec<KnowledgeGraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraphNode {
    pub id: String,
    pub label: String,
    pub page_type: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraphEdge {
    pub source: String,
    pub target: String,
    pub weight: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IngestCacheEntry {
    pub hash: String,
    pub pages_written: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VectorChunk {
    pub page_path: String,
    pub page_title: String,
    pub page_type: String,
    pub chunk_idx: usize,
    pub heading: String,
    pub chunk_text: String,
    pub vector: Vec<f32>,
}

#[derive(Debug)]
pub(crate) struct FileBlock {
    pub path: String,
    pub content: String,
}

#[derive(Debug)]
pub(crate) struct ChunkPiece {
    pub heading: String,
    pub text: String,
}

#[derive(Debug)]
pub(crate) struct PageMeta {
    pub title: String,
    pub page_type: String,
    pub tags: Vec<String>,
    pub updated: Option<String>,
}
