use std::sync::Arc;

use futures::stream::{self, StreamExt};
use tokio::sync::Semaphore;

use super::collection::collection_root_checked;
use super::embed::fetch_embedding;
use super::pages::{list_pages_inner, strip_frontmatter};
use super::types::{ChunkPiece, KnowledgeSettings, KnowledgeVectorStats, VectorChunk};
use super::utils::spawn_blocking_string;

const TARGET_CHUNK_CHARS: usize = 1_000;
const MAX_CHUNK_CHARS: usize = 1_500;
const OVERLAP_CHARS: usize = 200;

pub(crate) fn chunk_markdown(markdown: &str) -> Vec<ChunkPiece> {
    let mut chunks = Vec::new();
    let mut heading = String::new();
    let mut current = String::new();

    for line in markdown.lines() {
        if line.starts_with('#') {
            if !current.trim().is_empty() {
                push_sized_chunks(&mut chunks, &heading, &current);
                current.clear();
            }
            heading = line.trim_start_matches('#').trim().to_string();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        push_sized_chunks(&mut chunks, &heading, &current);
    }
    chunks
}

fn push_sized_chunks(out: &mut Vec<ChunkPiece>, heading: &str, text: &str) {
    let paragraphs = text.split("\n\n").collect::<Vec<_>>();
    let mut current = String::new();
    for paragraph in paragraphs {
        if current.len() + paragraph.len() + 2 > TARGET_CHUNK_CHARS && !current.is_empty() {
            out.push(ChunkPiece {
                heading: heading.to_string(),
                text: current.trim().to_string(),
            });
            current = overlap_tail(&current);
        }
        if paragraph.len() > MAX_CHUNK_CHARS {
            for slice in hard_slices(paragraph, MAX_CHUNK_CHARS) {
                out.push(ChunkPiece {
                    heading: heading.to_string(),
                    text: slice,
                });
            }
        } else {
            current.push_str(paragraph);
            current.push_str("\n\n");
        }
    }
    if !current.trim().is_empty() {
        out.push(ChunkPiece {
            heading: heading.to_string(),
            text: current.trim().to_string(),
        });
    }
}

fn overlap_tail(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= OVERLAP_CHARS {
        return text.to_string();
    }
    text.chars().skip(char_count - OVERLAP_CHARS).collect()
}

fn hard_slices(text: &str, max_chars: usize) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    chars
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

pub(crate) async fn reindex_collection_inner(
    collection: &super::types::KnowledgeCollection,
    settings: &KnowledgeSettings,
) -> Result<KnowledgeVectorStats, String> {
    if settings.embedding_model.url.trim().is_empty()
        || settings.embedding_model.model.trim().is_empty()
    {
        return Err("知识库 embedding 模型未配置，无法重建索引。".to_string());
    }
    let pages = {
        let collection = collection.clone();
        spawn_blocking_string(move || list_pages_inner(&collection)).await?
    };

    struct PendingChunk {
        page_path: String,
        chunk_idx: usize,
        heading: String,
        chunk_text: String,
    }

    let settings_ref = Arc::new(settings.clone());
    let semaphore = Arc::new(Semaphore::new(8));
    let dimension_ref = Arc::new(std::sync::Mutex::new(0usize));

    let chunks: Vec<VectorChunk> = stream::iter(pages)
        .map(|page| {
            let settings_ref = settings_ref.clone();
            let semaphore = semaphore.clone();
            let dimension_ref = dimension_ref.clone();
            async move {
                let page_chunks = {
                    let page_path = page.path.clone();
                    tokio::task::spawn_blocking(move || {
                        let content = std::fs::read_to_string(&page_path).unwrap_or_default();
                        let body = strip_frontmatter(&content).to_string();
                        chunk_markdown(&body)
                            .into_iter()
                            .enumerate()
                            .map(|(idx, chunk)| PendingChunk {
                                page_path: page_path.clone(),
                                chunk_idx: idx,
                                heading: chunk.heading,
                                chunk_text: chunk.text,
                            })
                            .collect::<Vec<_>>()
                    })
                    .await
                    .map_err(|e| e.to_string())?
                };

                let mut results = Vec::with_capacity(page_chunks.len());
                for pc in page_chunks {
                    let _permit = semaphore.acquire().await.map_err(|e| e.to_string())?;
                    let vector =
                        fetch_embedding(&pc.chunk_text, &settings_ref.embedding_model).await?;
                    let mut dim = dimension_ref.lock().map_err(|e| e.to_string())?;
                    if *dim == 0 {
                        *dim = vector.len();
                    } else if *dim != vector.len() {
                        return Err(format!(
                            "embedding 维度不一致：期望 {}，实际 {}",
                            *dim,
                            vector.len()
                        ));
                    }
                    drop(dim);
                    results.push(VectorChunk {
                        page_path: pc.page_path,
                        page_title: page.title.clone(),
                        page_type: page.page_type.clone(),
                        chunk_idx: pc.chunk_idx,
                        heading: pc.heading,
                        chunk_text: pc.chunk_text,
                        vector,
                    });
                }
                Ok(results)
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<Result<Vec<VectorChunk>, String>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    let _dimension = *dimension_ref.lock().map_err(|e| e.to_string())?;

    let root = collection_root_checked(collection).map_err(|error| error.to_string())?;
    let stored_chunks = chunks
        .into_iter()
        .map(|chunk| super::vector_store::StoredChunk {
            page_path: chunk.page_path,
            page_title: chunk.page_title,
            page_type: chunk.page_type,
            chunk_idx: chunk.chunk_idx,
            heading: chunk.heading,
            chunk_text: chunk.chunk_text,
            vector: chunk.vector,
        })
        .collect::<Vec<_>>();
    let stats = super::vector_store::replace_all_chunks(&root, stored_chunks)
        .await
        .map_err(|error| error.to_string())?;
    super::vector_store::drop_legacy_table(&root)
        .await
        .map_err(|error| error.to_string())?;
    super::cache::save_stats_cache(&root, &stats).map_err(|error| error.to_string())?;

    Ok(KnowledgeVectorStats {
        collection_id: collection.id.clone(),
        page_count: stats.page_count,
        chunk_count: stats.chunk_count,
        dimension: stats.dimension,
    })
}

#[tauri::command]
pub async fn knowledge_reindex_collection(
    collection_id: String,
) -> Result<KnowledgeVectorStats, String> {
    let collection =
        spawn_blocking_string(move || super::collection::find_collection(&collection_id)).await?;
    let settings = spawn_blocking_string(super::settings::load_settings).await?;
    reindex_collection_inner(&collection, &settings).await
}
