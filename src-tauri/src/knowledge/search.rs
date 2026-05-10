use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, Result};

use super::collection::collection_root_checked;
use super::embed::fetch_embedding;
use super::pages::{list_pages_inner, parse_page_meta, strip_frontmatter};
use super::types::{KnowledgeSearchResult, KnowledgePageSummary};
use super::utils::spawn_blocking_string;

fn token_rank_pages(
    pages: &[KnowledgePageSummary],
    query_lower: &str,
) -> HashMap<String, (usize, f32)> {
    let tokens = tokenize(query_lower);
    let mut scored = Vec::new();
    for page in pages {
        let haystack = format!(
            "{} {} {}",
            page.title.to_lowercase(),
            page.page_type,
            page.tags.join(" ")
        );
        let mut score = 0.0f32;
        if haystack.contains(query_lower) {
            score += 8.0;
        }
        for token in &tokens {
            if page.title.to_lowercase().contains(token) {
                score += 3.0;
            }
            score += haystack.matches(token).count() as f32 * 0.5;
        }
        if score > 0.0 {
            scored.push((page.path.clone(), score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .enumerate()
        .map(|(index, (path, score))| (path, (index + 1, score)))
        .collect()
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
    let q = query.to_lowercase();
    let lower = text.to_lowercase();
    let pos = lower.find(&q).unwrap_or(0);
    let char_count = text.chars().count();
    let start = text
        .char_indices()
        .nth(pos.saturating_sub(100))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = text
        .char_indices()
        .nth((pos + q.len() + 260).min(char_count))
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text[start..end].to_string()
}

async fn search_collection_inner(
    collection: &super::types::KnowledgeCollection,
    query: &str,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<KnowledgeSearchResult>> {
    let root = collection_root_checked(collection)?;
    let stats = super::vector_store::stats(&root).await?;
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

    let query_lower = query.to_lowercase();
    let pages = {
        let collection = collection.clone();
        spawn_blocking_string(move || list_pages_inner(&collection))
            .await
            .map_err(|error| anyhow!(error))?
    };
    let token_rank = token_rank_pages(&pages, &query_lower);

    let mut vector_scores: HashMap<String, (usize, f32, String)> = HashMap::new();
    let chunk_scores =
        super::vector_store::search_chunks(&root, query_vector.to_vec(), limit * 3).await?;
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

    // Batch-read all page contents in one spawn_blocking to avoid sync I/O in async context
    let content_cache: HashMap<String, String> = {
        let paths: Vec<String> = page_paths.iter().cloned().collect();
        spawn_blocking_string(move || {
            let mut map = HashMap::new();
            for path in paths {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                map.insert(path, content);
            }
            Ok(map)
        })
        .await
        .map_err(|error| anyhow!(error))?
    };

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
        let content = content_cache.get(&page_path).map(String::as_str).unwrap_or("");
        let meta = parse_page_meta(content, std::path::Path::new(&page_path));
        let snippet = vector_scores
            .get(&page_path)
            .map(|(_, _, chunk)| make_snippet(chunk, query))
            .unwrap_or_else(|| make_snippet(strip_frontmatter(content), query));
        results.push(KnowledgeSearchResult {
            collection_id: collection.id.clone(),
            collection_name: collection.name.clone(),
            path: page_path.clone(),
            relative_path: super::pages::relative_to_collection(
                collection,
                std::path::Path::new(&page_path),
            )?,
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

pub(crate) async fn search_collections(
    query: String,
    collection_ids: Option<Vec<String>>,
    limit: usize,
) -> Result<Vec<KnowledgeSearchResult>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let settings = spawn_blocking_string(super::settings::load_settings).await?;
    if settings.embedding_model.url.trim().is_empty()
        || settings.embedding_model.model.trim().is_empty()
    {
        return Err("知识库 embedding 模型未配置，无法执行混合检索。".to_string());
    }
    let query_vector = fetch_embedding(&query, &settings.embedding_model).await?;
    let collections = spawn_blocking_string(super::collection::load_collections).await?;
    let allowed = collection_ids
        .map(|ids| ids.into_iter().collect::<HashSet<_>>())
        .unwrap_or_default();

    let mut all_results = Vec::new();
    let collection_count = if allowed.is_empty() {
        collections.len()
    } else {
        collections.iter().filter(|c| allowed.contains(&c.id)).count()
    };
    let per_collection = if collection_count == 0 {
        return Ok(Vec::new());
    } else if collection_count <= 3 {
        limit * 2
    } else {
        limit
    };
    for collection in collections {
        if !allowed.is_empty() && !allowed.contains(&collection.id) {
            continue;
        }
        match search_collection_inner(&collection, &query, &query_vector, per_collection).await {
            Ok(mut results) => all_results.append(&mut results),
            Err(error) => {
                eprintln!(
                    "搜索集合 `{}` ({}) 失败：{}",
                    collection.name, collection.id, error
                );
                continue;
            }
        }
    }
    all_results.sort_by(|a, b| b.score.total_cmp(&a.score));
    all_results.truncate(limit);
    Ok(all_results)
}

#[tauri::command]
pub async fn knowledge_search(
    query: String,
    collection_ids: Option<Vec<String>>,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeSearchResult>, String> {
    search_collections(query, collection_ids, limit.unwrap_or(12).max(1)).await
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