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
    let path = collection_root
        .join(".llm-wiki")
        .join("stats-cache.json");
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
    let path = collection_root
        .join(".llm-wiki")
        .join("stats-cache.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

#[tauri::command]
pub async fn knowledge_vector_stats(
    collection_id: String,
) -> Result<KnowledgeVectorStats, String> {
    let collection = spawn_blocking_string(move || find_collection(&collection_id)).await?;
    let root =
        collection_root_checked(&collection).map_err(|error| error.to_string())?;
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