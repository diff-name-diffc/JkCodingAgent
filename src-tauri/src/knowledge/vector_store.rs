use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use anyhow::{anyhow, Result};
use arrow_array::{
    ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::connect;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use sha2::{Digest, Sha256};

const TABLE_V2: &str = "wiki_chunks_v2";
const TABLE_V1: &str = "wiki_vectors";

#[derive(Debug, Clone)]
pub struct StoredChunk {
    pub page_path: String,
    pub page_title: String,
    pub page_type: String,
    pub chunk_idx: usize,
    pub heading: String,
    pub chunk_text: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct ChunkSearchHit {
    pub page_path: String,
    pub chunk_text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanceVectorStats {
    pub page_count: usize,
    pub chunk_count: usize,
    pub dimension: usize,
}

fn db_path(collection_root: &Path) -> String {
    collection_root
        .join(".llm-wiki")
        .join("lancedb")
        .to_string_lossy()
        .replace('\\', "/")
}

pub async fn replace_all_chunks(
    collection_root: &Path,
    chunks: Vec<StoredChunk>,
) -> Result<LanceVectorStats> {
    let db = connect(&db_path(collection_root)).execute().await?;
    let tables = db.table_names().execute().await?;
    if tables.contains(&TABLE_V2.to_string()) {
        db.drop_table(TABLE_V2, &[]).await?;
    }

    if chunks.is_empty() {
        return Ok(LanceVectorStats::default());
    }

    let dimension = chunks[0].vector.len();
    if dimension == 0 {
        return Err(anyhow!("向量维度不能为空"));
    }
    let schema = make_schema(dimension as i32);
    let batch = make_batch(schema.clone(), &chunks, dimension as i32)?;
    db.create_table(TABLE_V2, vec![batch]).execute().await?;

    let page_count = chunks
        .iter()
        .map(|chunk| chunk.page_path.as_str())
        .collect::<HashSet<_>>()
        .len();
    Ok(LanceVectorStats {
        page_count,
        chunk_count: chunks.len(),
        dimension,
    })
}

pub async fn search_chunks(
    collection_root: &Path,
    query_vector: Vec<f32>,
    top_k: usize,
) -> Result<Vec<ChunkSearchHit>> {
    let db = connect(&db_path(collection_root)).execute().await?;
    let tables = db.table_names().execute().await?;
    if !tables.contains(&TABLE_V2.to_string()) {
        return Ok(Vec::new());
    }
    let table = db.open_table(TABLE_V2).execute().await?;
    let stream = table
        .vector_search(query_vector)?
        .limit(top_k)
        .execute()
        .await?;
    let batches: Vec<RecordBatch> = stream.try_collect().await?;
    let mut out = Vec::new();

    for batch in batches {
        let page_paths = string_column(&batch, "page_path")?;
        let chunk_texts = string_column(&batch, "chunk_text")?;
        let distances = batch
            .column_by_name("_distance")
            .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow!("LanceDB 缺少 _distance 列"))?;

        for i in 0..batch.num_rows() {
            let distance = distances.value(i);
            out.push(ChunkSearchHit {
                page_path: page_paths.value(i).to_string(),
                chunk_text: chunk_texts.value(i).to_string(),
                score: 1.0 / (1.0 + distance),
            });
        }
    }
    Ok(out)
}

pub async fn delete_page(collection_root: &Path, page_path: &str) -> Result<()> {
    let db = connect(&db_path(collection_root)).execute().await?;
    let tables = db.table_names().execute().await?;
    if !tables.contains(&TABLE_V2.to_string()) {
        return Ok(());
    }
    let table = db.open_table(TABLE_V2).execute().await?;
    table
        .delete(&format!("page_id = '{}'", page_id(page_path)))
        .await?;
    Ok(())
}

pub async fn stats(collection_root: &Path) -> Result<LanceVectorStats> {
    if let Some(cached) = crate::knowledge::cache::load_stats_cache(collection_root) {
        return Ok(cached);
    }

    let db = connect(&db_path(collection_root)).execute().await?;
    let tables = db.table_names().execute().await?;
    if !tables.contains(&TABLE_V2.to_string()) {
        return Ok(LanceVectorStats::default());
    }
    let table = db.open_table(TABLE_V2).execute().await?;
    let chunk_count = table.count_rows(None).await?;
    let dimension = table
        .schema()
        .await?
        .field_with_name("vector")
        .ok()
        .and_then(|field| match field.data_type() {
            DataType::FixedSizeList(_, dim) => Some(*dim as usize),
            _ => None,
        })
        .unwrap_or(0);

    let stream = table
        .query()
        .select(Select::columns(&["page_id"]))
        .execute()
        .await?;
    let batches: Vec<RecordBatch> = stream.try_collect().await?;
    let mut pages = HashSet::new();
    for batch in batches {
        let page_ids = string_column(&batch, "page_id")?;
        for i in 0..batch.num_rows() {
            pages.insert(page_ids.value(i).to_string());
        }
    }

    Ok(LanceVectorStats {
        page_count: pages.len(),
        chunk_count,
        dimension,
    })
}

pub async fn drop_legacy_table(collection_root: &Path) -> Result<()> {
    let db = connect(&db_path(collection_root)).execute().await?;
    let tables = db.table_names().execute().await?;
    if tables.contains(&TABLE_V1.to_string()) {
        db.drop_table(TABLE_V1, &[]).await?;
    }
    Ok(())
}

fn make_schema(dim: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("page_id", DataType::Utf8, false),
        Field::new("page_path", DataType::Utf8, false),
        Field::new("page_title", DataType::Utf8, false),
        Field::new("page_type", DataType::Utf8, false),
        Field::new("chunk_index", DataType::UInt32, false),
        Field::new("chunk_text", DataType::Utf8, false),
        Field::new("heading_path", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            false,
        ),
    ]))
}

fn make_batch(schema: Arc<Schema>, chunks: &[StoredChunk], dim: i32) -> Result<RecordBatch> {
    let mut chunk_ids = Vec::with_capacity(chunks.len());
    let mut page_ids = Vec::with_capacity(chunks.len());
    let mut page_paths = Vec::with_capacity(chunks.len());
    let mut page_titles = Vec::with_capacity(chunks.len());
    let mut page_types = Vec::with_capacity(chunks.len());
    let mut chunk_indexes = Vec::with_capacity(chunks.len());
    let mut chunk_texts = Vec::with_capacity(chunks.len());
    let mut heading_paths = Vec::with_capacity(chunks.len());
    let mut flat_vectors = Vec::with_capacity(chunks.len() * dim as usize);

    for chunk in chunks {
        if chunk.vector.len() as i32 != dim {
            return Err(anyhow!(
                "chunk #{} 维度 {} 与表维度 {} 不一致",
                chunk.chunk_idx,
                chunk.vector.len(),
                dim
            ));
        }
        let page_id = page_id(&chunk.page_path);
        chunk_ids.push(format!("{}#{}", page_id, chunk.chunk_idx));
        page_ids.push(page_id);
        page_paths.push(chunk.page_path.clone());
        page_titles.push(chunk.page_title.clone());
        page_types.push(chunk.page_type.clone());
        chunk_indexes.push(chunk.chunk_idx as u32);
        chunk_texts.push(chunk.chunk_text.clone());
        heading_paths.push(chunk.heading.clone());
        flat_vectors.extend_from_slice(&chunk.vector);
    }

    let values = Float32Array::from(flat_vectors);
    let vector: ArrayRef = Arc::new(FixedSizeListArray::new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        dim,
        Arc::new(values),
        None,
    ));

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(chunk_ids)) as ArrayRef,
            Arc::new(StringArray::from(page_ids)),
            Arc::new(StringArray::from(page_paths)),
            Arc::new(StringArray::from(page_titles)),
            Arc::new(StringArray::from(page_types)),
            Arc::new(UInt32Array::from(chunk_indexes)),
            Arc::new(StringArray::from(chunk_texts)),
            Arc::new(StringArray::from(heading_paths)),
            vector,
        ],
    )?)
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("LanceDB 缺少 {name} 列"))
}

fn page_id(page_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(page_path.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-lancedb-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join(".llm-wiki/lancedb")).unwrap();
        root
    }

    fn chunk(page: &str, idx: usize, seed: f32) -> StoredChunk {
        StoredChunk {
            page_path: page.to_string(),
            page_title: format!("Page {page}"),
            page_type: "concept".to_string(),
            chunk_idx: idx,
            heading: "Heading".to_string(),
            chunk_text: format!("chunk {idx}"),
            vector: vec![seed, 0.2, 0.3, 0.4],
        }
    }

    #[tokio::test]
    async fn replace_search_and_count_chunks() {
        let root = tmp_root();
        replace_all_chunks(
            &root,
            vec![chunk("wiki/a.md", 0, 0.1), chunk("wiki/b.md", 0, 0.9)],
        )
        .await
        .unwrap();

        let stats = stats(&root).await.unwrap();
        assert_eq!(stats.page_count, 2);
        assert_eq!(stats.chunk_count, 2);
        assert_eq!(stats.dimension, 4);

        let hits = search_chunks(&root, vec![0.1, 0.2, 0.3, 0.4], 2)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|hit| hit.page_path == "wiki/a.md"));
    }

    #[tokio::test]
    async fn delete_page_removes_its_chunks() {
        let root = tmp_root();
        replace_all_chunks(
            &root,
            vec![chunk("wiki/a.md", 0, 0.1), chunk("wiki/b.md", 0, 0.9)],
        )
        .await
        .unwrap();
        delete_page(&root, "wiki/a.md").await.unwrap();
        let stats = stats(&root).await.unwrap();
        assert_eq!(stats.page_count, 1);
        assert_eq!(stats.chunk_count, 1);
    }
}
