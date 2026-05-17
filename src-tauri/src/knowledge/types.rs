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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_collection_serde_roundtrip() {
        let collection = KnowledgeCollection {
            id: "kc-1234".to_string(),
            name: "Test Collection".to_string(),
            root_path: "/tmp/test".to_string(),
            created_at: 1000,
            updated_at: 2000,
        };
        let json = serde_json::to_string(&collection).unwrap();
        assert!(json.contains("\"rootPath\""));
        assert!(json.contains("\"createdAt\""));
        let parsed: KnowledgeCollection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, collection.id);
        assert_eq!(parsed.name, collection.name);
        assert_eq!(parsed.root_path, collection.root_path);
        assert_eq!(parsed.created_at, collection.created_at);
    }

    #[test]
    fn knowledge_collection_camelcase_serialization() {
        let collection = KnowledgeCollection {
            id: "kc-1".to_string(),
            name: "Test".to_string(),
            root_path: "/tmp".to_string(),
            created_at: 0,
            updated_at: 0,
        };
        let json = serde_json::to_string(&collection).unwrap();
        assert!(json.contains("rootPath"));
        assert!(json.contains("createdAt"));
        assert!(json.contains("updatedAt"));
        assert!(!json.contains("root_path"));
        assert!(!json.contains("created_at"));
    }

    #[test]
    fn knowledge_model_config_default() {
        let config = KnowledgeModelConfig::default();
        assert!(config.url.is_empty());
        assert!(config.api_key.is_empty());
        assert!(config.model.is_empty());
    }

    #[test]
    fn knowledge_settings_default() {
        let settings = KnowledgeSettings::default();
        assert!(settings.text_model.url.is_empty());
        assert!(settings.vision_model.url.is_empty());
        assert!(settings.embedding_model.url.is_empty());
    }

    #[test]
    fn knowledge_settings_serde_roundtrip() {
        let settings = KnowledgeSettings {
            text_model: KnowledgeModelConfig {
                url: "http://localhost:8000".to_string(),
                api_key: "secret".to_string(),
                model: "gpt-4".to_string(),
            },
            vision_model: KnowledgeModelConfig::default(),
            embedding_model: KnowledgeModelConfig {
                url: "http://embed".to_string(),
                api_key: String::new(),
                model: "embed-v1".to_string(),
            },
        };
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: KnowledgeSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text_model.url, "http://localhost:8000");
        assert_eq!(parsed.text_model.api_key, "secret");
        assert_eq!(parsed.embedding_model.model, "embed-v1");
        assert!(parsed.vision_model.url.is_empty());
    }

    #[test]
    fn knowledge_ingest_job_serde_roundtrip() {
        let job = KnowledgeIngestJob {
            id: "job-1".to_string(),
            collection_id: "kc-1".to_string(),
            source_name: "test.pdf".to_string(),
            source_path: "/tmp/test.pdf".to_string(),
            status: "running".to_string(),
            message: "Processing".to_string(),
            pages_written: vec!["wiki/a.md".to_string()],
            created_at: 1000,
            updated_at: 2000,
        };
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("\"sourceName\""));
        assert!(json.contains("\"sourcePath\""));
        assert!(json.contains("\"pagesWritten\""));
        let parsed: KnowledgeIngestJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, job.id);
        assert_eq!(parsed.pages_written, job.pages_written);
    }

    #[test]
    fn knowledge_page_summary_serde_roundtrip() {
        let summary = KnowledgePageSummary {
            collection_id: "kc-1".to_string(),
            path: "/tmp/wiki/a.md".to_string(),
            relative_path: "wiki/a.md".to_string(),
            title: "My Page".to_string(),
            page_type: "concept".to_string(),
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            updated: Some("2026-01-01".to_string()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let parsed: KnowledgePageSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, summary.title);
        assert_eq!(parsed.tags, summary.tags);
        assert_eq!(parsed.updated, Some("2026-01-01".to_string()));
    }

    #[test]
    fn knowledge_page_summary_none_updated() {
        let summary = KnowledgePageSummary {
            collection_id: "kc-1".to_string(),
            path: "/tmp/wiki/a.md".to_string(),
            relative_path: "wiki/a.md".to_string(),
            title: "Test".to_string(),
            page_type: "concept".to_string(),
            tags: vec![],
            updated: None,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("null"));
        let parsed: KnowledgePageSummary = serde_json::from_str(&json).unwrap();
        assert!(parsed.updated.is_none());
    }

    #[test]
    fn knowledge_search_result_serde_roundtrip() {
        let result = KnowledgeSearchResult {
            collection_id: "kc-1".to_string(),
            collection_name: "My KB".to_string(),
            path: "/tmp/wiki/a.md".to_string(),
            relative_path: "wiki/a.md".to_string(),
            title: "Test".to_string(),
            page_type: "concept".to_string(),
            snippet: "Some text...".to_string(),
            score: 0.95,
            vector_score: 0.9,
            token_score: 1.0,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: KnowledgeSearchResult = serde_json::from_str(&json).unwrap();
        assert!((parsed.score - 0.95).abs() < f32::EPSILON);
        assert!((parsed.vector_score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn knowledge_graph_serde_roundtrip() {
        let graph = KnowledgeGraph {
            nodes: vec![KnowledgeGraphNode {
                id: "node-1".to_string(),
                label: "Concept A".to_string(),
                page_type: "concept".to_string(),
                path: "wiki/a.md".to_string(),
            }],
            edges: vec![KnowledgeGraphEdge {
                source: "node-1".to_string(),
                target: "node-2".to_string(),
                weight: 3.5,
                reason: "wikilink".to_string(),
            }],
        };
        let json = serde_json::to_string(&graph).unwrap();
        let parsed: KnowledgeGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.edges.len(), 1);
        assert!((parsed.edges[0].weight - 3.5).abs() < f32::EPSILON);
    }

    #[test]
    fn vector_stats_serde_roundtrip() {
        let stats = KnowledgeVectorStats {
            collection_id: "kc-1".to_string(),
            page_count: 10,
            chunk_count: 42,
            dimension: 384,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let parsed: KnowledgeVectorStats = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.page_count, 10);
        assert_eq!(parsed.chunk_count, 42);
        assert_eq!(parsed.dimension, 384);
    }

    #[test]
    fn ingest_cache_entry_serde_roundtrip() {
        let entry = IngestCacheEntry {
            hash: "abc123".to_string(),
            pages_written: vec!["wiki/a.md".to_string(), "wiki/b.md".to_string()],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: IngestCacheEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.hash, "abc123");
        assert_eq!(parsed.pages_written.len(), 2);
    }

    #[test]
    fn vector_chunk_debug_format() {
        let chunk = VectorChunk {
            page_path: "wiki/a.md".to_string(),
            page_title: "Test".to_string(),
            page_type: "concept".to_string(),
            chunk_idx: 0,
            heading: "Intro".to_string(),
            chunk_text: "Hello".to_string(),
            vector: vec![0.1, 0.2],
        };
        let debug = format!("{:?}", chunk);
        assert!(debug.contains("wiki/a.md"));
        assert!(debug.contains("Hello"));
    }

    #[test]
    fn file_block_debug_format() {
        let block = FileBlock {
            path: "wiki/test.md".to_string(),
            content: "# Test".to_string(),
        };
        let debug = format!("{:?}", block);
        assert!(debug.contains("wiki/test.md"));
    }

    #[test]
    fn chunk_piece_debug_format() {
        let piece = ChunkPiece {
            heading: "Intro".to_string(),
            text: "Body text".to_string(),
        };
        let debug = format!("{:?}", piece);
        assert!(debug.contains("Intro"));
    }

    #[test]
    fn page_meta_debug_format() {
        let meta = PageMeta {
            title: "Test".to_string(),
            page_type: "concept".to_string(),
            tags: vec!["a".to_string()],
            updated: Some("2026-01-01".to_string()),
        };
        let debug = format!("{:?}", meta);
        assert!(debug.contains("concept"));
    }
}
