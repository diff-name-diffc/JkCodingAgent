use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent::llm::{ChatMessage, OpenAiCompatProvider};
use crate::project::atomic_write;

mod document;
mod vector_store;

const COLLECTIONS_FILE: &str = "collections.json";
const SETTINGS_FILE: &str = "settings.json";
const INGEST_JOBS_FILE: &str = "ingest-jobs.json";
const INGEST_CACHE_FILE: &str = "ingest-cache.json";
const MAX_SOURCE_CHARS: usize = 120_000;
const MAX_CHUNK_CHARS: usize = 1_500;
const TARGET_CHUNK_CHARS: usize = 1_000;
const OVERLAP_CHARS: usize = 200;
const MAX_IMAGE_CAPTIONS_PER_SOURCE: usize = 50;

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
struct IngestCacheEntry {
    hash: String,
    pages_written: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VectorChunk {
    page_path: String,
    page_title: String,
    page_type: String,
    chunk_idx: usize,
    heading: String,
    chunk_text: String,
    vector: Vec<f32>,
}

pub fn set_resource_dir_hint(dir: PathBuf) {
    document::set_resource_dir_hint(dir);
}

#[tauri::command]
pub async fn knowledge_list_collections() -> Result<Vec<KnowledgeCollection>, String> {
    spawn_blocking_string(load_collections).await
}

#[tauri::command]
pub async fn knowledge_create_collection(name: String) -> Result<KnowledgeCollection, String> {
    spawn_blocking_string(move || create_collection_inner(&name)).await
}

#[tauri::command]
pub async fn knowledge_update_collection(
    collection_id: String,
    name: String,
) -> Result<KnowledgeCollection, String> {
    spawn_blocking_string(move || {
        let mut collections = load_collections()?;
        let now = now_ms();
        let Some(collection) = collections.iter_mut().find(|item| item.id == collection_id) else {
            return Err(anyhow!("知识库集合不存在：{collection_id}"));
        };
        let clean_name = clean_collection_name(&name)?;
        collection.name = clean_name;
        collection.updated_at = now;
        let updated = collection.clone();
        save_collections(&collections)?;
        Ok(updated)
    })
    .await
}

#[tauri::command]
pub async fn knowledge_delete_collection(collection_id: String) -> Result<(), String> {
    spawn_blocking_string(move || {
        let mut collections = load_collections()?;
        let Some(index) = collections.iter().position(|item| item.id == collection_id) else {
            return Err(anyhow!("知识库集合不存在：{collection_id}"));
        };
        let collection = collections.remove(index);
        let root = collection_root_checked(&collection)?;
        if root.exists() {
            fs::remove_dir_all(&root)
                .with_context(|| format!("删除集合目录失败：{}", root.display()))?;
        }
        save_collections(&collections)
    })
    .await
}

#[tauri::command]
pub async fn knowledge_get_settings() -> Result<KnowledgeSettings, String> {
    spawn_blocking_string(load_settings).await
}

#[tauri::command]
pub async fn knowledge_save_settings(
    settings: KnowledgeSettings,
) -> Result<KnowledgeSettings, String> {
    spawn_blocking_string(move || {
        let path = settings_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&settings)?;
        atomic_write(&path, &raw).map_err(|error| anyhow!(error))?;
        Ok(settings)
    })
    .await
}

#[tauri::command]
pub async fn knowledge_test_model(
    kind: String,
    settings: KnowledgeSettings,
) -> Result<String, String> {
    match kind.as_str() {
        "embedding" => {
            let vector = fetch_embedding("ping", &settings.embedding_model).await?;
            Ok(format!("embedding ok，维度 {}", vector.len()))
        }
        "text" => {
            let content = call_text_model(
                &settings.text_model,
                "你是一个连通性测试助手，只输出 pong。",
                "ping",
            )
            .await?;
            Ok(format!("text ok：{}", truncate_chars(content.trim(), 120)))
        }
        "vision" => {
            let model = if settings.vision_model.model.trim().is_empty()
                && settings.vision_model.url.trim().is_empty()
            {
                &settings.text_model
            } else {
                &settings.vision_model
            };
            let content = call_text_model(model, "只输出 pong。", "ping").await?;
            Ok(format!(
                "vision endpoint ok：{}",
                truncate_chars(content.trim(), 120)
            ))
        }
        _ => Err(format!("未知模型类型：{kind}")),
    }
}

#[tauri::command]
pub async fn knowledge_import_sources(
    collection_id: String,
    paths: Vec<String>,
) -> Result<Vec<KnowledgeIngestJob>, String> {
    let settings = load_settings().map_err(|error| error.to_string())?;
    if settings.text_model.url.trim().is_empty() || settings.text_model.model.trim().is_empty() {
        return Err("知识库文本模型未配置，无法导入。".to_string());
    }
    let collection = find_collection(&collection_id).map_err(|error| error.to_string())?;
    let mut jobs = Vec::new();

    for path in paths {
        let job = ingest_one_source(collection.clone(), path, settings.clone()).await?;
        jobs.push(job);
    }

    Ok(jobs)
}

#[tauri::command]
pub async fn knowledge_get_ingest_jobs(
    collection_id: Option<String>,
) -> Result<Vec<KnowledgeIngestJob>, String> {
    spawn_blocking_string(move || {
        let mut jobs = load_ingest_jobs()?;
        if let Some(collection_id) = collection_id {
            jobs.retain(|job| job.collection_id == collection_id);
        }
        jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(jobs)
    })
    .await
}

#[tauri::command]
pub async fn knowledge_cancel_ingest(job_id: String) -> Result<KnowledgeIngestJob, String> {
    spawn_blocking_string(move || update_job_status(&job_id, "cancelled", "任务已取消")).await
}

#[tauri::command]
pub async fn knowledge_retry_ingest(job_id: String) -> Result<KnowledgeIngestJob, String> {
    let job = spawn_blocking_string({
        let job_id = job_id.clone();
        move || {
            load_ingest_jobs()?
                .into_iter()
                .find(|job| job.id == job_id)
                .ok_or_else(|| anyhow!("导入任务不存在：{job_id}"))
        }
    })
    .await?;
    knowledge_import_sources(job.collection_id, vec![job.source_path])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "重试任务未产生结果".to_string())
}

#[tauri::command]
pub async fn knowledge_list_pages(
    collection_id: String,
) -> Result<Vec<KnowledgePageSummary>, String> {
    spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        list_pages_inner(&collection)
    })
    .await
}

#[tauri::command]
pub async fn knowledge_read_page(
    collection_id: String,
    relative_path: String,
) -> Result<KnowledgePageContent, String> {
    spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        let page_path = resolve_collection_relative_path(&collection, &relative_path)?;
        ensure_wiki_markdown_path(&collection, &page_path)?;
        let content = fs::read_to_string(&page_path)
            .with_context(|| format!("读取页面失败：{}", page_path.display()))?;
        let meta = parse_page_meta(&content, &page_path);
        Ok(KnowledgePageContent {
            collection_id,
            path: normalize_path_string(&page_path),
            relative_path: relative_to_collection(&collection, &page_path)?,
            title: meta.title,
            content,
        })
    })
    .await
}

#[tauri::command]
pub async fn knowledge_write_page(
    collection_id: String,
    relative_path: String,
    content: String,
) -> Result<KnowledgePageContent, String> {
    spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        let page_path = resolve_collection_relative_path(&collection, &relative_path)?;
        ensure_wiki_markdown_path(&collection, &page_path)?;
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&page_path, &content).map_err(|error| anyhow!(error))?;
        touch_collection(&collection.id)?;
        let meta = parse_page_meta(&content, &page_path);
        Ok(KnowledgePageContent {
            collection_id,
            path: normalize_path_string(&page_path),
            relative_path: relative_to_collection(&collection, &page_path)?,
            title: meta.title,
            content,
        })
    })
    .await
}

#[tauri::command]
pub async fn knowledge_delete_page(
    collection_id: String,
    relative_path: String,
) -> Result<(), String> {
    let (collection, page_path) = spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        let page_path = resolve_collection_relative_path(&collection, &relative_path)?;
        ensure_wiki_markdown_path(&collection, &page_path)?;
        fs::remove_file(&page_path)
            .with_context(|| format!("删除页面失败：{}", page_path.display()))?;
        touch_collection(&collection.id)?;
        Ok((collection, normalize_path_string(&page_path)))
    })
    .await?;
    let root = collection_root_checked(&collection).map_err(|error| error.to_string())?;
    vector_store::delete_page(&root, &page_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn knowledge_reindex_collection(
    collection_id: String,
) -> Result<KnowledgeVectorStats, String> {
    let collection = find_collection(&collection_id).map_err(|error| error.to_string())?;
    let settings = load_settings().map_err(|error| error.to_string())?;
    reindex_collection_inner(&collection, &settings).await
}

#[tauri::command]
pub async fn knowledge_vector_stats(collection_id: String) -> Result<KnowledgeVectorStats, String> {
    let collection = spawn_blocking_string(move || find_collection(&collection_id)).await?;
    let root = collection_root_checked(&collection).map_err(|error| error.to_string())?;
    let stats = vector_store::stats(&root)
        .await
        .map_err(|error| error.to_string())?;
    Ok(KnowledgeVectorStats {
        collection_id: collection.id,
        page_count: stats.page_count,
        chunk_count: stats.chunk_count,
        dimension: stats.dimension,
    })
}

#[tauri::command]
pub async fn knowledge_search(
    query: String,
    collection_ids: Option<Vec<String>>,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeSearchResult>, String> {
    search_collections(query, collection_ids, limit.unwrap_or(12).max(1)).await
}

#[tauri::command]
pub async fn knowledge_build_graph(collection_id: String) -> Result<KnowledgeGraph, String> {
    spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        build_graph_inner(&collection)
    })
    .await
}

pub async fn search_for_agent(
    query: String,
    collection_ids: Option<Vec<String>>,
    limit: usize,
) -> String {
    match search_collections(query, collection_ids, limit.max(1)).await {
        Ok(results) if results.is_empty() => "知识库没有命中结果。".to_string(),
        Ok(results) => results
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{}. [{}] {} ({}) score={:.3}\n{}\npath: {}",
                    index + 1,
                    item.collection_name,
                    item.title,
                    item.page_type,
                    item.score,
                    item.snippet,
                    item.relative_path,
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        Err(error) => format!("知识库检索失败：{error}"),
    }
}

pub fn read_page_for_agent(
    collection_id: String,
    relative_path: String,
    max_chars: usize,
) -> String {
    match read_page_for_agent_inner(&collection_id, &relative_path, max_chars.max(500)) {
        Ok(content) => content,
        Err(error) => format!("读取知识库页面失败：{error}"),
    }
}

async fn ingest_one_source(
    collection: KnowledgeCollection,
    source_path: String,
    settings: KnowledgeSettings,
) -> Result<KnowledgeIngestJob, String> {
    let source_path_buf = PathBuf::from(&source_path);
    if !source_path_buf.is_absolute() {
        return Err("导入源文件必须是绝对路径。".to_string());
    }
    if !source_path_buf.is_file() {
        return Err(format!("导入源文件不存在或不是文件：{source_path}"));
    }

    let source_name = source_path_buf
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "源文件名不是有效 UTF-8".to_string())?
        .to_string();
    let now = now_ms();
    let mut job = KnowledgeIngestJob {
        id: uuid::Uuid::new_v4().to_string(),
        collection_id: collection.id.clone(),
        source_name: source_name.clone(),
        source_path: source_path.clone(),
        status: "running".to_string(),
        message: "正在解析".to_string(),
        pages_written: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    upsert_ingest_job(job.clone()).map_err(|error| error.to_string())?;

    let (copied_source, hash, extraction) = {
        let collection_for_extract = collection.clone();
        let source_path_for_extract = source_path_buf.clone();
        spawn_blocking_string(move || {
            let copied_source =
                copy_source_into_collection(&collection_for_extract, &source_path_for_extract)?;
            let hash = hash_file(&copied_source)?;
            let collection_root = collection_root_checked(&collection_for_extract)?;
            let extraction = document::extract_source_content(&copied_source, &collection_root)?;
            Ok((copied_source, hash, extraction))
        })
        .await
        .map_err(|error| fail_job(job.clone(), anyhow!(error)))?
    };
    let source_key = normalize_path_string(&copied_source);
    let cache = load_ingest_cache(&collection).map_err(|error| fail_job(job.clone(), error))?;
    if let Some(entry) = cache.get(&source_key) {
        if entry.hash == hash && entry.pages_written.iter().all(|p| Path::new(p).exists()) {
            job.status = "skipped".to_string();
            job.message = "源文件未变化，已跳过".to_string();
            job.pages_written = entry.pages_written.clone();
            job.updated_at = now_ms();
            upsert_ingest_job(job.clone()).map_err(|error| error.to_string())?;
            return Ok(job);
        }
    }

    let source_text = enrich_source_text_with_image_captions(&extraction, &settings)
        .await
        .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
    let model_output = call_text_model(
        &settings.text_model,
        &analysis_system_prompt(),
        &build_ingest_prompt(&source_name, &source_text),
    )
    .await
    .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
    let blocks = parse_file_blocks(&model_output).map_err(|error| fail_job(job.clone(), error))?;
    if blocks.is_empty() {
        let error = anyhow!("LLM 未返回 FILE 块，导入中止。");
        return Err(fail_job(job, error));
    }

    let mut written = Vec::new();
    for block in blocks {
        let page_path = resolve_generated_page_path(&collection, &block.path)
            .map_err(|error| fail_job(job.clone(), error))?;
        let content = prepare_page_content(&block.content, &source_name, &page_path)
            .map_err(|error| fail_job(job.clone(), error))?;
        let final_content = if page_path.exists() {
            let existing = fs::read_to_string(&page_path)
                .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
            merge_existing_page(&settings.text_model, &existing, &content, &source_name)
                .await
                .map_err(|error| fail_job(job.clone(), anyhow!(error)))?
        } else {
            content
        };
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent).map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
        }
        atomic_write(&page_path, &final_content)
            .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
        written.push(normalize_path_string(&page_path));
    }

    save_ingest_cache_entry(&collection, source_key, hash, written.clone())
        .map_err(|error| fail_job(job.clone(), error))?;

    if !settings.embedding_model.url.trim().is_empty()
        && !settings.embedding_model.model.trim().is_empty()
    {
        if let Err(error) = reindex_collection_inner(&collection, &settings).await {
            return Err(fail_job(job, anyhow!(error)));
        }
    }

    touch_collection(&collection.id).map_err(|error| fail_job(job.clone(), error))?;
    job.status = "done".to_string();
    job.message = format!("写入 {} 个页面", written.len());
    job.pages_written = written;
    job.updated_at = now_ms();
    upsert_ingest_job(job.clone()).map_err(|error| error.to_string())?;
    Ok(job)
}

fn fail_job(mut job: KnowledgeIngestJob, error: anyhow::Error) -> String {
    job.status = "failed".to_string();
    job.message = error.to_string();
    job.updated_at = now_ms();
    let _ = upsert_ingest_job(job);
    error.to_string()
}

async fn enrich_source_text_with_image_captions(
    extraction: &document::DocumentExtraction,
    settings: &KnowledgeSettings,
) -> Result<String, String> {
    let mut source_text = truncate_chars(&extraction.markdown, MAX_SOURCE_CHARS);
    if extraction.images.is_empty() {
        return Ok(source_text);
    }

    let Some(model) = configured_vision_model(settings) else {
        return Ok(source_text);
    };

    let mut caption_blocks = Vec::new();
    for image in extraction.images.iter().take(MAX_IMAGE_CAPTIONS_PER_SOURCE) {
        let data_url = image_data_url(image).await?;
        let prompt = format!(
            "请为这张知识库导入图片生成简体中文 caption。\n\
要求：\n\
- 一句话概括图片内容。\n\
- 如果是图表、表格、截图，提取关键文字、数值、实体和关系。\n\
- 不要杜撰看不见的信息。\n\n\
图片路径：{}\n图片：![image]({})",
            image.rel_path, data_url
        );
        let caption = call_text_model(
            &model,
            "你是知识库图片标注助手，只输出可检索的图片说明。",
            &prompt,
        )
        .await?;
        caption_blocks.push(format!(
            "![Image {}]({})\n\nCaption: {}",
            image.index,
            image.rel_path,
            caption.trim()
        ));
    }

    if !caption_blocks.is_empty() {
        source_text.push_str("\n\n## Image Captions\n\n");
        source_text.push_str(&caption_blocks.join("\n\n"));
        source_text.push('\n');
    }
    Ok(truncate_chars(&source_text, MAX_SOURCE_CHARS))
}

fn configured_vision_model(settings: &KnowledgeSettings) -> Option<KnowledgeModelConfig> {
    let model = &settings.vision_model;
    if model.url.trim().is_empty() || model.model.trim().is_empty() {
        None
    } else {
        Some(model.clone())
    }
}

async fn image_data_url(image: &document::SavedImage) -> Result<String, String> {
    let image = image.clone();
    spawn_blocking_string(move || {
        let bytes = fs::read(&image.abs_path)
            .with_context(|| format!("读取知识库图片失败：{}", image.abs_path))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(anyhow!(
                "图片超过 8MB，拒绝发送给多模态模型：{}",
                image.abs_path
            ));
        }
        Ok(format!(
            "data:{};base64,{}",
            image.mime_type,
            BASE64.encode(bytes)
        ))
    })
    .await
}

async fn reindex_collection_inner(
    collection: &KnowledgeCollection,
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

    let mut chunks = Vec::new();
    let mut dimension = 0usize;
    for page in pages {
        let content = fs::read_to_string(&page.path).map_err(|error| error.to_string())?;
        let body = strip_frontmatter(&content);
        for (idx, chunk) in chunk_markdown(body).into_iter().enumerate() {
            let vector = fetch_embedding(&chunk.text, &settings.embedding_model).await?;
            if dimension == 0 {
                dimension = vector.len();
            } else if dimension != vector.len() {
                return Err(format!(
                    "embedding 维度不一致：期望 {dimension}，实际 {}",
                    vector.len()
                ));
            }
            chunks.push(VectorChunk {
                page_path: page.path.clone(),
                page_title: page.title.clone(),
                page_type: page.page_type.clone(),
                chunk_idx: idx,
                heading: chunk.heading,
                chunk_text: chunk.text,
                vector,
            });
        }
    }

    let root = collection_root_checked(collection).map_err(|error| error.to_string())?;
    let stored_chunks = chunks
        .into_iter()
        .map(|chunk| vector_store::StoredChunk {
            page_path: chunk.page_path,
            page_title: chunk.page_title,
            page_type: chunk.page_type,
            chunk_idx: chunk.chunk_idx,
            heading: chunk.heading,
            chunk_text: chunk.chunk_text,
            vector: chunk.vector,
        })
        .collect::<Vec<_>>();
    let stats = vector_store::replace_all_chunks(&root, stored_chunks)
        .await
        .map_err(|error| error.to_string())?;
    vector_store::drop_legacy_table(&root)
        .await
        .map_err(|error| error.to_string())?;

    Ok(KnowledgeVectorStats {
        collection_id: collection.id.clone(),
        page_count: stats.page_count,
        chunk_count: stats.chunk_count,
        dimension: stats.dimension,
    })
}

async fn search_collections(
    query: String,
    collection_ids: Option<Vec<String>>,
    limit: usize,
) -> Result<Vec<KnowledgeSearchResult>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let settings = load_settings().map_err(|error| error.to_string())?;
    if settings.embedding_model.url.trim().is_empty()
        || settings.embedding_model.model.trim().is_empty()
    {
        return Err("知识库 embedding 模型未配置，无法执行混合检索。".to_string());
    }
    let query_vector = fetch_embedding(&query, &settings.embedding_model).await?;
    let collections = load_collections().map_err(|error| error.to_string())?;
    let allowed = collection_ids
        .map(|ids| ids.into_iter().collect::<HashSet<_>>())
        .unwrap_or_default();

    let mut all_results = Vec::new();
    for collection in collections {
        if !allowed.is_empty() && !allowed.contains(&collection.id) {
            continue;
        }
        let mut results = search_collection_inner(&collection, &query, &query_vector, limit * 3)
            .await
            .map_err(|error| error.to_string())?;
        all_results.append(&mut results);
    }
    all_results.sort_by(|a, b| b.score.total_cmp(&a.score));
    all_results.truncate(limit);
    Ok(all_results)
}

async fn search_collection_inner(
    collection: &KnowledgeCollection,
    query: &str,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<KnowledgeSearchResult>> {
    let root = collection_root_checked(collection)?;
    let stats = vector_store::stats(&root).await?;
    if stats.chunk_count == 0 {
        return Err(anyhow!(
            "集合 `{}` 没有向量索引，请先导入并重建索引。",
            collection.name
        ));
    }
    if stats.dimension != query_vector.len() {
        return Err(anyhow!(
            "查询 embedding 维度 {} 与索引维度 {} 不一致。",
            query_vector.len(),
            stats.dimension
        ));
    }
    let collection_for_token = collection.clone();
    let query_for_token = query.to_string();
    let token_rank =
        spawn_blocking_string(move || token_rank_pages(&collection_for_token, &query_for_token))
            .await
            .map_err(|error| anyhow!(error))?;
    let mut vector_scores: HashMap<String, (usize, f32, String)> = HashMap::new();
    let chunk_scores = vector_store::search_chunks(&root, query_vector.to_vec(), limit * 3).await?;
    for (rank, chunk) in chunk_scores.into_iter().enumerate() {
        let page_path = chunk.page_path;
        let chunk_text = chunk.chunk_text;
        let score = chunk.score;
        let entry = vector_scores
            .entry(page_path)
            .or_insert((rank + 1, score, chunk_text.clone()));
        if score > entry.1 {
            *entry = (rank + 1, score, chunk_text);
        }
    }

    let mut page_paths = token_rank.keys().cloned().collect::<BTreeSet<_>>();
    page_paths.extend(vector_scores.keys().cloned());

    let mut results = Vec::new();
    for page_path in page_paths {
        let token_rank_no = token_rank.get(&page_path).map(|(rank, _)| *rank);
        let vector_rank_no = vector_scores.get(&page_path).map(|(rank, _, _)| *rank);
        let token_score = token_rank
            .get(&page_path)
            .map(|(_, score)| *score)
            .unwrap_or(0.0);
        let vector_score = vector_scores
            .get(&page_path)
            .map(|(_, score, _)| *score)
            .unwrap_or(0.0);
        let rrf = token_rank_no.map(rrf_score).unwrap_or(0.0)
            + vector_rank_no.map(rrf_score).unwrap_or(0.0);
        let content = fs::read_to_string(&page_path).unwrap_or_default();
        let meta = parse_page_meta(&content, Path::new(&page_path));
        let snippet = vector_scores
            .get(&page_path)
            .map(|(_, _, chunk)| make_snippet(chunk, query))
            .unwrap_or_else(|| make_snippet(strip_frontmatter(&content), query));
        results.push(KnowledgeSearchResult {
            collection_id: collection.id.clone(),
            collection_name: collection.name.clone(),
            path: page_path.clone(),
            relative_path: relative_to_collection(collection, Path::new(&page_path))?,
            title: meta.title,
            page_type: meta.page_type,
            snippet,
            score: rrf + vector_score * 0.01 + token_score * 0.001,
            vector_score,
            token_score,
        });
    }
    results.sort_by(|a, b| b.score.total_cmp(&a.score));
    results.truncate(limit);
    Ok(results)
}

fn build_graph_inner(collection: &KnowledgeCollection) -> Result<KnowledgeGraph> {
    let pages = list_pages_inner(collection)?;
    let mut slug_to_path = HashMap::new();
    let mut path_to_title = HashMap::new();
    let mut path_to_type = HashMap::new();
    let mut path_to_sources = HashMap::new();
    let mut path_to_links = HashMap::new();

    for page in &pages {
        let content = fs::read_to_string(&page.path).unwrap_or_default();
        let slug = page_slug(&page.path);
        slug_to_path.insert(slug.clone(), page.path.clone());
        slug_to_path.insert(slugify(&page.title), page.path.clone());
        path_to_title.insert(page.path.clone(), page.title.clone());
        path_to_type.insert(page.path.clone(), page.page_type.clone());
        path_to_sources.insert(
            page.path.clone(),
            extract_frontmatter_array(&content, "sources"),
        );
        path_to_links.insert(page.path.clone(), extract_wikilinks(&content));
    }

    let nodes = pages
        .iter()
        .map(|page| KnowledgeGraphNode {
            id: page.path.clone(),
            label: page.title.clone(),
            page_type: page.page_type.clone(),
            path: page.relative_path.clone(),
        })
        .collect::<Vec<_>>();

    let mut edge_weights: BTreeMap<(String, String), (f32, String)> = BTreeMap::new();
    for (source_path, links) in &path_to_links {
        for link in links {
            let key = slugify(link);
            if let Some(target_path) = slug_to_path.get(&key) {
                if target_path != source_path {
                    let pair = ordered_pair(source_path, target_path);
                    add_edge_weight(&mut edge_weights, pair, 3.0, "wikilink");
                }
            }
        }
    }
    for i in 0..pages.len() {
        for j in (i + 1)..pages.len() {
            let a = &pages[i].path;
            let b = &pages[j].path;
            let a_sources = path_to_sources.get(a).cloned().unwrap_or_default();
            let b_sources = path_to_sources.get(b).cloned().unwrap_or_default();
            if !a_sources.is_empty() && !b_sources.is_empty() {
                let overlap = a_sources.intersection(&b_sources).count();
                if overlap > 0 {
                    add_edge_weight(
                        &mut edge_weights,
                        ordered_pair(a, b),
                        1.0 + overlap as f32,
                        "source-overlap",
                    );
                }
            }
        }
    }

    let edges = edge_weights
        .into_iter()
        .map(|((source, target), (weight, reason))| KnowledgeGraphEdge {
            source,
            target,
            weight,
            reason,
        })
        .collect();
    Ok(KnowledgeGraph { nodes, edges })
}

fn read_page_for_agent_inner(
    collection_id: &str,
    relative_path: &str,
    max_chars: usize,
) -> Result<String> {
    let collection = find_collection(collection_id)?;
    let page_path = resolve_collection_relative_path(&collection, relative_path)?;
    ensure_wiki_markdown_path(&collection, &page_path)?;
    let content = fs::read_to_string(&page_path)?;
    let trimmed = truncate_chars(&content, max_chars);
    Ok(format!(
        "[{}] {}\npath: {}\n\n{}",
        collection.name,
        parse_page_meta(&content, &page_path).title,
        relative_to_collection(&collection, &page_path)?,
        trimmed
    ))
}

async fn call_text_model(
    model: &KnowledgeModelConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    let base = normalize_chat_base_url(&model.url);
    let provider =
        OpenAiCompatProvider::new(model.api_key.clone(), base, model.model.clone(), 8192, 0.1);
    let messages = vec![
        ChatMessage::system(system_prompt.to_string()),
        ChatMessage {
            role: "user".to_string(),
            content: user_prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ];
    let mut _sink = String::new();
    provider
        .chat_stream(&messages, &[], true, |delta| _sink.push_str(delta))
        .await
        .map(|response| response.content)
        .map_err(|error| error.to_string())
}

async fn fetch_embedding(text: &str, model: &KnowledgeModelConfig) -> Result<Vec<f32>, String> {
    let endpoint = normalize_embedding_endpoint(&model.url);
    let client = reqwest::Client::new();
    let mut input = text.trim().to_string();
    if input.is_empty() {
        input = " ".to_string();
    }
    for _ in 0..5 {
        let mut request = client.post(&endpoint).json(&json!({
            "model": model.model,
            "input": input,
        }));
        if !model.api_key.trim().is_empty() {
            request = request.bearer_auth(model.api_key.trim());
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().await.map_err(|error| error.to_string())?;
        if status.is_success() {
            let value: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            return parse_embedding_response(&value)
                .ok_or_else(|| "embedding 响应中未找到 data[0].embedding".to_string());
        }
        let lower = body.to_lowercase();
        if input.len() > 200
            && (lower.contains("too long")
                || lower.contains("maximum context")
                || lower.contains("tokens"))
        {
            input.truncate(input.len() / 2);
            continue;
        }
        return Err(format!("embedding 请求失败，HTTP {status}：{body}"));
    }
    Err("embedding 文本过长，自动减半重试后仍失败。".to_string())
}

fn parse_embedding_response(value: &Value) -> Option<Vec<f32>> {
    value
        .get("data")?
        .as_array()?
        .first()?
        .get("embedding")?
        .as_array()?
        .iter()
        .map(|item| item.as_f64().map(|v| v as f32))
        .collect()
}

fn normalize_chat_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/chat/completions")
        .or_else(|| trimmed.strip_suffix("/v1/chat/completions"))
        .unwrap_or(trimmed)
        .to_string()
}

fn normalize_embedding_endpoint(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.ends_with("/embeddings") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{trimmed}/embeddings")
    } else {
        format!("{trimmed}/v1/embeddings")
    }
}

fn create_collection_inner(name: &str) -> Result<KnowledgeCollection> {
    let clean_name = clean_collection_name(name)?;
    let mut collections = load_collections()?;
    let now = now_ms();
    let id = format!("kc-{}", uuid::Uuid::new_v4());
    let root = collections_root()?.join(&id);
    create_collection_dirs(&root)?;
    let collection = KnowledgeCollection {
        id,
        name: clean_name,
        root_path: normalize_path_string(&root),
        created_at: now,
        updated_at: now,
    };
    collections.push(collection.clone());
    save_collections(&collections)?;
    Ok(collection)
}

fn create_collection_dirs(root: &Path) -> Result<()> {
    for rel in [
        "raw/sources",
        "raw/assets",
        "wiki/entities",
        "wiki/concepts",
        "wiki/sources",
        "wiki/queries",
        "wiki/comparisons",
        "wiki/synthesis",
        "wiki/media",
        ".llm-wiki/lancedb",
    ] {
        fs::create_dir_all(root.join(rel))?;
    }
    let overview = root.join("wiki/overview.md");
    if !overview.exists() {
        atomic_write(
            &overview,
            "---\ntype: overview\ntitle: Overview\n---\n\n# Overview\n",
        )
        .map_err(|error| anyhow!(error))?;
    }
    Ok(())
}

fn load_collections() -> Result<Vec<KnowledgeCollection>> {
    let path = collections_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)?;
    let mut collections: Vec<KnowledgeCollection> = serde_json::from_str(&raw)?;
    collections.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(collections)
}

fn save_collections(collections: &[KnowledgeCollection]) -> Result<()> {
    let path = collections_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, &serde_json::to_string_pretty(collections)?).map_err(|error| anyhow!(error))
}

fn find_collection(collection_id: &str) -> Result<KnowledgeCollection> {
    load_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or_else(|| anyhow!("知识库集合不存在：{collection_id}"))
        .and_then(|collection| {
            collection_root_checked(&collection)?;
            Ok(collection)
        })
}

fn touch_collection(collection_id: &str) -> Result<()> {
    let mut collections = load_collections()?;
    let Some(collection) = collections.iter_mut().find(|item| item.id == collection_id) else {
        return Err(anyhow!("知识库集合不存在：{collection_id}"));
    };
    collection.updated_at = now_ms();
    save_collections(&collections)
}

fn load_settings() -> Result<KnowledgeSettings> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(KnowledgeSettings::default());
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn collections_root() -> Result<PathBuf> {
    Ok(knowledge_root()?.join("collections"))
}

fn knowledge_root() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("找不到用户主目录"))?;
    Ok(home.join(".jkcodingagent").join("knowledge"))
}

fn collections_path() -> Result<PathBuf> {
    Ok(knowledge_root()?.join(COLLECTIONS_FILE))
}

fn settings_path() -> Result<PathBuf> {
    Ok(knowledge_root()?.join(SETTINGS_FILE))
}

fn collection_root_checked(collection: &KnowledgeCollection) -> Result<PathBuf> {
    let root = PathBuf::from(&collection.root_path);
    if !root.is_absolute() {
        return Err(anyhow!("集合路径必须是绝对路径：{}", collection.root_path));
    }
    let canonical_parent = root
        .parent()
        .ok_or_else(|| anyhow!("集合路径缺少父目录"))?
        .canonicalize()
        .or_else(|_| collections_root())?;
    let canonical_collections = collections_root()?
        .canonicalize()
        .or_else(|_| collections_root())?;
    if !canonical_parent.starts_with(&canonical_collections) {
        return Err(anyhow!("集合路径不在应用知识库目录内：{}", root.display()));
    }
    Ok(root)
}

fn resolve_collection_relative_path(
    collection: &KnowledgeCollection,
    relative_path: &str,
) -> Result<PathBuf> {
    if relative_path.contains("..") {
        return Err(anyhow!("路径不能包含 .."));
    }
    let root = collection_root_checked(collection)?;
    let path = root.join(relative_path.trim_start_matches('/'));
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("路径缺少父目录"))?
        .to_path_buf();
    if parent.exists() {
        let canonical_parent = parent.canonicalize()?;
        let canonical_root = root.canonicalize()?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(anyhow!("路径越界：{relative_path}"));
        }
    }
    Ok(path)
}

fn ensure_wiki_markdown_path(collection: &KnowledgeCollection, path: &Path) -> Result<()> {
    let root = collection_root_checked(collection)?.join("wiki");
    let canonical_root = root
        .canonicalize()
        .or_else(|_| Ok::<_, anyhow::Error>(root.clone()))?;
    let parent = path.parent().ok_or_else(|| anyhow!("页面路径缺少父目录"))?;
    let canonical_parent = parent
        .canonicalize()
        .or_else(|_| Ok::<_, anyhow::Error>(parent.to_path_buf()))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(anyhow!("页面必须位于 wiki/ 目录内"));
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return Err(anyhow!("页面必须是 .md 文件"));
    }
    Ok(())
}

fn copy_source_into_collection(collection: &KnowledgeCollection, source: &Path) -> Result<PathBuf> {
    let root = collection_root_checked(collection)?;
    let dest_dir = root.join("raw/sources");
    fs::create_dir_all(&dest_dir)?;
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("源文件缺少文件名"))?;
    let dest = dest_dir.join(file_name);
    fs::copy(source, &dest)
        .with_context(|| format!("复制源文件失败：{} -> {}", source.display(), dest.display()))?;
    Ok(dest)
}

fn analysis_system_prompt() -> String {
    "你是知识库迁移管线。请把输入资料整理成高质量 Markdown Wiki 页面。必须只输出若干 FILE 块；每个块形如：\nFILE: wiki/concepts/example.md\n```markdown\n---\ntype: concept\ntitle: Example\nsources: [\"source.ext\"]\ntags: []\ncreated: 2026-05-09\nupdated: 2026-05-09\n---\n\n# Example\n...\n```\n不要输出解释。".to_string()
}

fn build_ingest_prompt(source_name: &str, source_text: &str) -> String {
    format!(
        "源文件：{source_name}\n\n要求：\n- 使用简体中文。\n- 优先生成 concept/source/entity 类型页面。\n- 页面之间可以使用 [[页面标题]] 形成知识链接。\n- 每个页面必须有 YAML frontmatter，sources 必须包含源文件名。\n- 只输出 FILE 块。\n\n源内容：\n{}",
        source_text
    )
}

fn merge_prompt(existing: &str, incoming: &str, source_name: &str) -> String {
    format!(
        "把已有 Wiki 页面和新导入内容合并为一个完整 Markdown 文件。\n\
要求：保留事实、不要缩短信息量、sources/tags/related 做并集、保留已有 created/title/type，updated 写今天。只输出完整 Markdown。\n\n\
源文件：{source_name}\n\n【已有页面】\n{existing}\n\n【新内容】\n{incoming}"
    )
}

async fn merge_existing_page(
    model: &KnowledgeModelConfig,
    existing: &str,
    incoming: &str,
    source_name: &str,
) -> Result<String, String> {
    let merged = call_text_model(
        model,
        "你负责安全合并 Markdown Wiki 页面，只输出完整页面内容。",
        &merge_prompt(existing, incoming, source_name),
    )
    .await?;
    let sanitized = sanitize_markdown_page(&merged);
    if !sanitized.trim_start().starts_with("---") {
        return Err("LLM 合并结果缺少 frontmatter，已拒绝写入。".to_string());
    }
    Ok(sanitized)
}

struct FileBlock {
    path: String,
    content: String,
}

fn parse_file_blocks(output: &str) -> Result<Vec<FileBlock>> {
    let mut blocks = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed
            .strip_prefix("FILE:")
            .or_else(|| trimmed.strip_prefix("File:"))
        {
            if let Some(path) = current_path.take() {
                blocks.push(FileBlock {
                    path,
                    content: current.join("\n"),
                });
                current.clear();
            }
            current_path = Some(path.trim().to_string());
            continue;
        }
        if trimmed == "```"
            || trimmed == "```markdown"
            || trimmed == "```md"
            || trimmed == "```yaml"
        {
            continue;
        }
        if current_path.is_some() {
            current.push(line.to_string());
        }
    }
    if let Some(path) = current_path {
        blocks.push(FileBlock {
            path,
            content: current.join("\n"),
        });
    }
    Ok(blocks
        .into_iter()
        .filter(|block| !block.path.trim().is_empty() && !block.content.trim().is_empty())
        .collect())
}

fn resolve_generated_page_path(
    collection: &KnowledgeCollection,
    raw_path: &str,
) -> Result<PathBuf> {
    let mut rel = raw_path.trim().replace('\\', "/");
    rel = rel.trim_start_matches('/').to_string();
    if rel.contains("..") {
        return Err(anyhow!("LLM 返回的页面路径包含 ..：{raw_path}"));
    }
    if !rel.starts_with("wiki/") {
        rel = format!("wiki/concepts/{rel}");
    }
    if !rel.ends_with(".md") {
        rel.push_str(".md");
    }
    let path = collection_root_checked(collection)?.join(rel);
    ensure_wiki_markdown_path(collection, &path)?;
    Ok(path)
}

fn prepare_page_content(content: &str, source_name: &str, page_path: &Path) -> Result<String> {
    let mut content = sanitize_markdown_page(content);
    if !content.trim_start().starts_with("---") {
        let title = page_path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(title_from_slug)
            .unwrap_or_else(|| "Untitled".to_string());
        content = format!(
            "---\ntype: concept\ntitle: {}\nsources: [\"{}\"]\ntags: []\ncreated: {}\nupdated: {}\n---\n\n{}",
            yaml_escape(&title),
            yaml_escape(source_name),
            today(),
            today(),
            content.trim()
        );
    } else if !content.contains("sources:") {
        content = content.replacen(
            "---\n",
            &format!("---\nsources: [\"{}\"]\n", yaml_escape(source_name)),
            1,
        );
    }
    Ok(content)
}

fn sanitize_markdown_page(content: &str) -> String {
    let mut s = content.trim().to_string();
    for prefix in ["```markdown", "```md", "```yaml", "```"] {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].trim_start().to_string();
        }
    }
    if s.ends_with("```") {
        s.truncate(s.len() - 3);
    }
    s.trim().to_string() + "\n"
}

#[derive(Debug)]
struct ChunkPiece {
    heading: String,
    text: String,
}

fn chunk_markdown(markdown: &str) -> Vec<ChunkPiece> {
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
    if text.len() <= OVERLAP_CHARS {
        return text.to_string();
    }
    text.chars()
        .rev()
        .take(OVERLAP_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
}

fn hard_slices(text: &str, max_chars: usize) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    chars
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

fn list_pages_inner(collection: &KnowledgeCollection) -> Result<Vec<KnowledgePageSummary>> {
    let root = collection_root_checked(collection)?;
    let wiki = root.join("wiki");
    let mut paths = Vec::new();
    collect_md_files(&wiki, &mut paths)?;
    let mut pages = Vec::new();
    for path in paths {
        if path
            .components()
            .any(|component| component.as_os_str() == "media")
        {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        let meta = parse_page_meta(&content, &path);
        pages.push(KnowledgePageSummary {
            collection_id: collection.id.clone(),
            path: normalize_path_string(&path),
            relative_path: relative_to_collection(collection, &path)?,
            title: meta.title,
            page_type: meta.page_type,
            tags: meta.tags,
            updated: meta.updated,
        });
    }
    pages.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(pages)
}

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PageMeta {
    title: String,
    page_type: String,
    tags: Vec<String>,
    updated: Option<String>,
}

fn parse_page_meta(content: &str, path: &Path) -> PageMeta {
    let frontmatter = frontmatter_block(content).unwrap_or_default();
    PageMeta {
        title: extract_frontmatter_string(frontmatter, "title").unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(title_from_slug)
                .unwrap_or_else(|| "Untitled".to_string())
        }),
        page_type: extract_frontmatter_string(frontmatter, "type")
            .unwrap_or_else(|| "concept".to_string()),
        tags: extract_frontmatter_array(content, "tags")
            .into_iter()
            .collect(),
        updated: extract_frontmatter_string(frontmatter, "updated"),
    }
}

fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn strip_frontmatter(content: &str) -> &str {
    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    content
}

fn extract_frontmatter_string(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| trim_yaml_scalar(v))
    })
}

fn extract_frontmatter_array(content: &str, key: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    let Some(frontmatter) = frontmatter_block(content) else {
        return values;
    };
    let mut in_block = false;
    for line in frontmatter.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(&format!("{key}:")) {
            let rest = rest.trim();
            if rest.starts_with('[') && rest.ends_with(']') {
                for item in rest.trim_matches(['[', ']']).split(',') {
                    let item = trim_yaml_scalar(item);
                    if !item.is_empty() {
                        values.insert(item);
                    }
                }
                in_block = false;
            } else {
                in_block = true;
            }
            continue;
        }
        if in_block {
            let trimmed = line.trim_start();
            if let Some(item) = trimmed.strip_prefix("- ") {
                let item = trim_yaml_scalar(item);
                if !item.is_empty() {
                    values.insert(item);
                }
            } else if !line.starts_with(' ') && !line.starts_with('\t') {
                in_block = false;
            }
        }
    }
    values
}

fn trim_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn load_ingest_jobs() -> Result<Vec<KnowledgeIngestJob>> {
    let path = knowledge_root()?.join(INGEST_JOBS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn save_ingest_jobs(jobs: &[KnowledgeIngestJob]) -> Result<()> {
    let path = knowledge_root()?.join(INGEST_JOBS_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, &serde_json::to_string_pretty(jobs)?).map_err(|error| anyhow!(error))
}

fn upsert_ingest_job(job: KnowledgeIngestJob) -> Result<()> {
    let mut jobs = load_ingest_jobs()?;
    if let Some(existing) = jobs.iter_mut().find(|item| item.id == job.id) {
        *existing = job;
    } else {
        jobs.push(job);
    }
    save_ingest_jobs(&jobs)
}

fn update_job_status(job_id: &str, status: &str, message: &str) -> Result<KnowledgeIngestJob> {
    let mut jobs = load_ingest_jobs()?;
    let Some(job) = jobs.iter_mut().find(|item| item.id == job_id) else {
        return Err(anyhow!("导入任务不存在：{job_id}"));
    };
    job.status = status.to_string();
    job.message = message.to_string();
    job.updated_at = now_ms();
    let updated = job.clone();
    save_ingest_jobs(&jobs)?;
    Ok(updated)
}

fn load_ingest_cache(
    collection: &KnowledgeCollection,
) -> Result<HashMap<String, IngestCacheEntry>> {
    let path = collection_root_checked(collection)?
        .join(".llm-wiki")
        .join(INGEST_CACHE_FILE);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn save_ingest_cache_entry(
    collection: &KnowledgeCollection,
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
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, &serde_json::to_string_pretty(&cache)?).map_err(|error| anyhow!(error))
}

fn token_rank_pages(
    collection: &KnowledgeCollection,
    query: &str,
) -> Result<HashMap<String, (usize, f32)>> {
    let pages = list_pages_inner(collection)?;
    let tokens = tokenize(query);
    let mut scored = Vec::new();
    for page in pages {
        let content = fs::read_to_string(&page.path).unwrap_or_default();
        let haystack = format!("{} {} {}", page.title, page.page_type, content).to_lowercase();
        let mut score = 0.0f32;
        if haystack.contains(&query.to_lowercase()) {
            score += 8.0;
        }
        for token in &tokens {
            if page.title.to_lowercase().contains(token) {
                score += 3.0;
            }
            score += haystack.matches(token).count() as f32 * 0.5;
        }
        if score > 0.0 {
            scored.push((page.path, score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(scored
        .into_iter()
        .enumerate()
        .map(|(index, (path, score))| (path, (index + 1, score)))
        .collect())
}

fn tokenize(query: &str) -> Vec<String> {
    let lower = query.to_lowercase();
    let mut tokens = lower
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|item| item.chars().count() >= 2)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let chars = lower.chars().collect::<Vec<_>>();
    if chars.iter().any(|c| !c.is_ascii()) {
        for pair in chars.windows(2) {
            tokens.push(pair.iter().collect());
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn rrf_score(rank: usize) -> f32 {
    1.0 / (60.0 + rank as f32)
}

fn make_snippet(text: &str, query: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let q = query.to_lowercase();
    let pos = compact.to_lowercase().find(&q).unwrap_or(0);
    let char_positions = compact
        .char_indices()
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    let char_pos = char_positions
        .iter()
        .position(|idx| *idx >= pos)
        .unwrap_or(0);
    let start_char = char_pos.saturating_sub(100);
    let end_char = (char_pos + 260).min(compact.chars().count());
    compact
        .chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect()
}

fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let target = after[..end].split('|').next().unwrap_or("").trim();
        if !target.is_empty() {
            links.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    links
}

fn add_edge_weight(
    edges: &mut BTreeMap<(String, String), (f32, String)>,
    pair: (String, String),
    weight: f32,
    reason: &str,
) {
    let entry = edges.entry(pair).or_insert((0.0, reason.to_string()));
    entry.0 += weight;
    if !entry.1.contains(reason) {
        entry.1.push_str(", ");
        entry.1.push_str(reason);
    }
}

fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn clean_collection_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("集合名称不能为空"));
    }
    if name.chars().count() > 80 {
        return Err(anyhow!("集合名称不能超过 80 个字符"));
    }
    Ok(name.to_string())
}

fn relative_to_collection(collection: &KnowledgeCollection, path: &Path) -> Result<String> {
    let root = collection_root_checked(collection)?;
    let rel = path.strip_prefix(root)?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

fn normalize_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn page_slug(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(slugify)
        .unwrap_or_default()
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn title_from_slug(slug: &str) -> String {
    slug.replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn today() -> String {
    Utc::now().date_naive().to_string()
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push_str("\n...[truncated]");
    }
    out
}

async fn spawn_blocking_string<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_embedding_response_reads_openai_shape() {
        let value = json!({ "data": [{ "embedding": [0.1, 0.2, 0.3] }] });
        assert_eq!(
            parse_embedding_response(&value).unwrap(),
            vec![0.1, 0.2, 0.3]
        );
    }

    #[test]
    fn file_block_parser_keeps_multiple_pages() {
        let output =
            "FILE: wiki/concepts/a.md\n```markdown\n# A\n```\nFILE: wiki/entities/b.md\n# B";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].path, "wiki/concepts/a.md");
        assert!(blocks[1].content.contains("# B"));
    }

    #[test]
    fn chunker_splits_large_text() {
        let text = format!("# A\n\n{}", "hello world. ".repeat(500));
        let chunks = chunk_markdown(&text);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| !chunk.text.trim().is_empty()));
    }

    #[test]
    fn snippet_handles_cjk_boundaries() {
        let snippet = make_snippet("前置内容。知识库检索需要支持中文边界。后续内容。", "检索");
        assert!(snippet.contains("知识库检索"));
    }
}
