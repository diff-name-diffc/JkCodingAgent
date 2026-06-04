use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};

use super::collection::{collection_root_checked, find_collection};
use super::types::{IngestCacheEntry, KnowledgeVectorStats};
use super::utils::spawn_blocking_string;

const INGEST_CACHE_FILE: &str = "ingest-cache.json";

pub(crate) fn load_ingest_cache(
    collection: &super::types::KnowledgeCollection,
) -> Result<HashMap<String, IngestCacheEntry>> {
    let path = collection_root_checked(collection)?
        .join(".llm-wiki")
        .join(INGEST_CACHE_FILE);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub(crate) fn save_ingest_cache_entry(
    collection: &super::types::KnowledgeCollection,
    source_key: String,
    hash: String,
    pages_written: Vec<String>,
) -> Result<()> {
    let mut cache = load_ingest_cache(collection)?;
    cache.insert(
        source_key,
        IngestCacheEntry {
            hash,
            pages_written,
        },
    );
    let path = collection_root_checked(collection)?
        .join(".llm-wiki")
        .join(INGEST_CACHE_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::project::atomic_write(&path, &serde_json::to_string_pretty(&cache)?)
        .map_err(|error| anyhow!(error))
}

pub(crate) fn save_stats_cache(
    collection_root: &Path,
    stats: &super::vector_store::LanceVectorStats,
) -> Result<()> {
    let path = collection_root.join(".llm-wiki").join("stats-cache.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(stats)?;
    crate::project::atomic_write(&path, &json).map_err(|error| anyhow!(error))?;
    Ok(())
}

pub(crate) fn load_stats_cache(
    collection_root: &Path,
) -> Option<super::vector_store::LanceVectorStats> {
    let path = collection_root.join(".llm-wiki").join("stats-cache.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

#[tauri::command]
pub async fn knowledge_vector_stats(collection_id: String) -> Result<KnowledgeVectorStats, String> {
    let collection = spawn_blocking_string(move || find_collection(&collection_id)).await?;
    let root = collection_root_checked(&collection).map_err(|error| error.to_string())?;
    let stats = super::vector_store::stats(&root)
        .await
        .map_err(|error| error.to_string())?;
    Ok(KnowledgeVectorStats {
        collection_id: collection.id,
        page_count: stats.page_count,
        chunk_count: stats.chunk_count,
        dimension: stats.dimension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_collection_root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-cache-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".llm-wiki")).unwrap();
        root
    }

    fn tmp_collection(root: &Path) -> super::super::types::KnowledgeCollection {
        super::super::types::KnowledgeCollection {
            id: format!("kc-test-{}", uuid::Uuid::new_v4()),
            name: "Test".to_string(),
            root_path: root.to_string_lossy().replace('\\', "/"),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn load_ingest_cache_missing_file() {
        let root = tmp_collection_root();
        let _collection = tmp_collection(&root);
        let cache_path = root.join(".llm-wiki").join("ingest-cache.json");
        assert!(!cache_path.exists());
        let result: std::collections::HashMap<String, IngestCacheEntry> =
            std::collections::HashMap::new();
        assert!(result.is_empty());
    }

    #[test]
    fn save_and_load_stats_cache() {
        let root = tmp_collection_root();
        let stats = super::super::vector_store::LanceVectorStats {
            page_count: 5,
            chunk_count: 20,
            dimension: 384,
        };
        save_stats_cache(&root, &stats).unwrap();

        let loaded = load_stats_cache(&root);
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.page_count, 5);
        assert_eq!(loaded.chunk_count, 20);
        assert_eq!(loaded.dimension, 384);
    }

    #[test]
    fn load_stats_cache_missing_returns_none() {
        let root = tmp_collection_root();
        assert!(load_stats_cache(&root).is_none());
    }

    #[test]
    fn save_stats_cache_creates_directory() {
        let root = tmp_collection_root();
        let subdir = root.join(".llm-wiki");
        // Remove to verify creation
        std::fs::remove_dir_all(&subdir).ok();
        assert!(!subdir.exists());

        let stats = super::super::vector_store::LanceVectorStats {
            page_count: 1,
            chunk_count: 2,
            dimension: 4,
        };
        save_stats_cache(&root, &stats).unwrap();
        assert!(subdir.exists());
        let loaded = load_stats_cache(&root);
        assert!(loaded.is_some());
    }

    #[test]
    fn stats_cache_overwrite() {
        let root = tmp_collection_root();
        let stats1 = super::super::vector_store::LanceVectorStats {
            page_count: 1,
            chunk_count: 1,
            dimension: 4,
        };
        save_stats_cache(&root, &stats1).unwrap();

        let stats2 = super::super::vector_store::LanceVectorStats {
            page_count: 99,
            chunk_count: 100,
            dimension: 768,
        };
        save_stats_cache(&root, &stats2).unwrap();

        let loaded = load_stats_cache(&root).unwrap();
        assert_eq!(loaded.page_count, 99);
        assert_eq!(loaded.chunk_count, 100);
        assert_eq!(loaded.dimension, 768);
    }

    #[test]
    fn ingest_cache_direct_roundtrip() {
        let root = tmp_collection_root();
        let cache_path = root.join(".llm-wiki").join("ingest-cache.json");

        let mut cache = std::collections::HashMap::new();
        cache.insert(
            "source-key-1".to_string(),
            IngestCacheEntry {
                hash: "abc123".to_string(),
                pages_written: vec!["wiki/a.md".to_string()],
            },
        );
        std::fs::write(&cache_path, serde_json::to_string_pretty(&cache).unwrap()).unwrap();

        let loaded: std::collections::HashMap<String, IngestCacheEntry> =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert_eq!(loaded.len(), 1);
        let entry = loaded.get("source-key-1").unwrap();
        assert_eq!(entry.hash, "abc123");
        assert_eq!(entry.pages_written, vec!["wiki/a.md"]);
    }

    #[test]
    fn ingest_cache_multiple_entries() {
        let root = tmp_collection_root();
        let cache_path = root.join(".llm-wiki").join("ingest-cache.json");

        let mut cache = std::collections::HashMap::new();
        cache.insert(
            "key-a".to_string(),
            IngestCacheEntry {
                hash: "hash-a".to_string(),
                pages_written: vec!["wiki/a.md".to_string()],
            },
        );
        cache.insert(
            "key-b".to_string(),
            IngestCacheEntry {
                hash: "hash-b".to_string(),
                pages_written: vec!["wiki/b.md".to_string(), "wiki/c.md".to_string()],
            },
        );
        std::fs::write(&cache_path, serde_json::to_string_pretty(&cache).unwrap()).unwrap();

        let loaded: std::collections::HashMap<String, IngestCacheEntry> =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains_key("key-a"));
        assert!(loaded.contains_key("key-b"));
    }
}
