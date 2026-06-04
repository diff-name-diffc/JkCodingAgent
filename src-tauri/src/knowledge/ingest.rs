use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use super::collection::{collection_root_checked, touch_collection};
use super::embed::{call_text_model, enrich_source_text_with_image_captions};
use super::types::{FileBlock, KnowledgeCollection, KnowledgeIngestJob, KnowledgeSettings};
use super::utils::{
    is_cancelled, normalize_path_string, now_ms, remove_cancel_token, spawn_blocking_string,
    title_from_slug, today, yaml_escape,
};

fn analysis_system_prompt() -> String {
    let today = today();
    format!(
        "你是知识库迁移管线。请把输入资料整理成高质量 Markdown Wiki 页面。必须只输出若干 FILE 块；每个块形如：\nFILE: wiki/concepts/example.md\n```markdown\n---\ntype: concept\ntitle: Example\nsources: [\"source.ext\"]\ntags: []\ncreated: {today}\nupdated: {today}\n---\n\n# Example\n...\n```\n不要输出解释。"
    )
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
    model: &super::types::KnowledgeModelConfig,
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

fn prepare_page_content(content: &str, source_name: &str, page_path: &Path) -> Result<String> {
    let mut content = sanitize_markdown_page(content);
    if !content.trim_start().starts_with("---") {
        let title = page_path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(title_from_slug)
            .unwrap_or_else(|| "Untitled".to_string());
        let today = today();
        content = format!(
            "---\ntype: concept\ntitle: {}\nsources: [\"{}\"]\ntags: []\ncreated: {today}\nupdated: {today}\n---\n\n{}",
            yaml_escape(&title),
            yaml_escape(source_name),
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

fn fail_job(mut job: KnowledgeIngestJob, error: anyhow::Error) -> String {
    job.status = "failed".to_string();
    job.message = error.to_string();
    job.updated_at = now_ms();
    remove_cancel_token(&job.id);
    std::thread::spawn(move || {
        let _ = super::jobs::upsert_ingest_job(job);
    });
    error.to_string()
}

fn cancel_job(mut job: KnowledgeIngestJob) -> String {
    job.status = "cancelled".to_string();
    job.message = "任务已取消".to_string();
    job.updated_at = now_ms();
    remove_cancel_token(&job.id);
    std::thread::spawn(move || {
        let _ = super::jobs::upsert_ingest_job(job);
    });
    "任务已取消".to_string()
}

pub(crate) async fn ingest_one_source_with_job(
    collection: KnowledgeCollection,
    source_path: String,
    settings: KnowledgeSettings,
    job_id: String,
) -> Result<KnowledgeIngestJob, String> {
    ingest_one_source_inner(collection, source_path, settings, Some(job_id)).await
}

async fn ingest_one_source(
    collection: KnowledgeCollection,
    source_path: String,
    settings: KnowledgeSettings,
) -> Result<KnowledgeIngestJob, String> {
    ingest_one_source_inner(collection, source_path, settings, None).await
}

pub(crate) async fn ingest_one_source_inner(
    collection: KnowledgeCollection,
    source_path: String,
    settings: KnowledgeSettings,
    existing_job_id: Option<String>,
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
    let job_id = existing_job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = now_ms();
    let mut job = KnowledgeIngestJob {
        id: job_id,
        collection_id: collection.id.clone(),
        source_name: source_name.clone(),
        source_path: source_path.clone(),
        status: "running".to_string(),
        message: "正在解析".to_string(),
        pages_written: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    spawn_blocking_string({
        let job = job.clone();
        move || super::jobs::upsert_ingest_job(job)
    })
    .await?;

    let (copied_source, hash, extraction) = {
        let collection_for_extract = collection.clone();
        let source_path_for_extract = source_path_buf.clone();
        spawn_blocking_string(move || {
            let copied_source =
                copy_source_into_collection(&collection_for_extract, &source_path_for_extract)?;
            let hash = hash_file(&copied_source)?;
            let collection_root = collection_root_checked(&collection_for_extract)?;
            let extraction =
                super::document::extract_source_content(&copied_source, &collection_root)?;
            Ok((copied_source, hash, extraction))
        })
        .await
        .map_err(|error| fail_job(job.clone(), anyhow!(error)))?
    };

    if is_cancelled(&job.id) {
        return Err(cancel_job(job));
    }

    let source_key = normalize_path_string(&copied_source);
    let cache = spawn_blocking_string({
        let collection = collection.clone();
        move || super::cache::load_ingest_cache(&collection)
    })
    .await
    .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
    if let Some(entry) = cache.get(&source_key) {
        if entry.hash == hash && entry.pages_written.iter().all(|p| Path::new(p).exists()) {
            job.status = "skipped".to_string();
            job.message = "源文件未变化，已跳过".to_string();
            job.pages_written = entry.pages_written.clone();
            job.updated_at = now_ms();
            spawn_blocking_string({
                let job = job.clone();
                move || super::jobs::upsert_ingest_job(job)
            })
            .await?;
            remove_cancel_token(&job.id);
            return Ok(job);
        }
    }

    let source_text = enrich_source_text_with_image_captions(&extraction, &settings)
        .await
        .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;

    if is_cancelled(&job.id) {
        return Err(cancel_job(job));
    }

    let model_output = call_text_model(
        &settings.text_model,
        &analysis_system_prompt(),
        &build_ingest_prompt(&source_name, &source_text),
    )
    .await
    .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;

    if is_cancelled(&job.id) {
        return Err(cancel_job(job));
    }

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
        crate::project::atomic_write(&page_path, &final_content)
            .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
        written.push(normalize_path_string(&page_path));
    }

    spawn_blocking_string({
        let collection = collection.clone();
        let source_key = source_key.clone();
        let hash = hash.clone();
        let written = written.clone();
        move || super::cache::save_ingest_cache_entry(&collection, source_key, hash, written)
    })
    .await
    .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;

    if !settings.embedding_model.url.trim().is_empty()
        && !settings.embedding_model.model.trim().is_empty()
    {
        if is_cancelled(&job.id) {
            return Err(cancel_job(job));
        }
        if let Err(error) = super::chunk::reindex_collection_inner(&collection, &settings).await {
            return Err(fail_job(job, anyhow!(error)));
        }
    }

    spawn_blocking_string({
        let collection_id = collection.id.clone();
        move || touch_collection(&collection_id)
    })
    .await
    .map_err(|error| fail_job(job.clone(), anyhow!(error)))?;
    job.status = "done".to_string();
    job.message = format!("写入 {} 个页面", written.len());
    job.pages_written = written;
    job.updated_at = now_ms();
    spawn_blocking_string({
        let job = job.clone();
        move || super::jobs::upsert_ingest_job(job)
    })
    .await?;
    remove_cancel_token(&job.id);
    Ok(job)
}

#[tauri::command]
pub async fn knowledge_import_sources(
    collection_id: String,
    paths: Vec<String>,
) -> Result<Vec<KnowledgeIngestJob>, String> {
    let settings = spawn_blocking_string(super::settings::load_settings).await?;
    if settings.text_model.url.trim().is_empty() || settings.text_model.model.trim().is_empty() {
        return Err("知识库文本模型未配置，无法导入。".to_string());
    }
    let collection =
        spawn_blocking_string(move || super::collection::find_collection(&collection_id)).await?;
    let mut jobs = Vec::new();

    for path in paths {
        let job = ingest_one_source(collection.clone(), path, settings.clone()).await?;
        jobs.push(job);
    }

    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_blocks_basic() {
        let output = "FILE: wiki/concepts/test.md\n```markdown\n# Test\nContent\n```\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "wiki/concepts/test.md");
        assert!(blocks[0].content.contains("# Test"));
    }

    #[test]
    fn parse_file_blocks_multiple() {
        let output = "\
FILE: wiki/a.md\nContent A\n\nFILE: wiki/b.md\nContent B\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].path, "wiki/a.md");
        assert_eq!(blocks[1].path, "wiki/b.md");
    }

    #[test]
    fn parse_file_blocks_case_insensitive() {
        let output = "File: wiki/lower.md\nLower content\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "wiki/lower.md");
    }

    #[test]
    fn parse_file_blocks_filters_empty_path() {
        let output = "FILE: \nSome content\n\nFILE: wiki/valid.md\nValid\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "wiki/valid.md");
    }

    #[test]
    fn parse_file_blocks_filters_empty_content() {
        let output = "FILE: wiki/empty.md\n\nFILE: wiki/notempty.md\nHas content\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "wiki/notempty.md");
    }

    #[test]
    fn parse_file_blocks_no_blocks() {
        let output = "Just some text without FILE markers";
        let blocks = parse_file_blocks(output).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn parse_file_blocks_last_block_no_trailing_newline() {
        let output = "FILE: wiki/test.md\n# Title\nBody text";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("Body text"));
    }

    #[test]
    fn sanitize_markdown_page_strips_code_fence() {
        let content = "```markdown\n# Title\nBody\n```";
        let sanitized = sanitize_markdown_page(content);
        assert!(!sanitized.starts_with("```"));
        assert!(!sanitized.ends_with("```"));
        assert!(sanitized.contains("# Title"));
    }

    #[test]
    fn sanitize_markdown_page_strips_yaml_fence() {
        let content = "```yaml\n---\ntitle: Test\n---\n```";
        let sanitized = sanitize_markdown_page(content);
        assert!(!sanitized.starts_with("```yaml"));
    }

    #[test]
    fn sanitize_markdown_page_strips_md_fence() {
        let content = "```md\n# Hello\n```";
        let sanitized = sanitize_markdown_page(content);
        assert!(sanitized.contains("# Hello"));
        assert!(!sanitized.starts_with("```md"));
    }

    #[test]
    fn sanitize_markdown_page_trailing_newline() {
        let content = "# Title";
        let sanitized = sanitize_markdown_page(content);
        assert!(sanitized.ends_with('\n'));
    }

    #[test]
    fn sanitize_markdown_page_already_clean() {
        let content = "# Title\n\nParagraph";
        let sanitized = sanitize_markdown_page(content);
        assert_eq!(sanitized, "# Title\n\nParagraph\n");
    }

    #[test]
    fn sanitize_markdown_page_trims_whitespace() {
        let content = "  \n  # Title  \n  ";
        let sanitized = sanitize_markdown_page(content);
        assert!(sanitized.starts_with('#'));
    }

    #[test]
    fn prepare_page_content_adds_frontmatter() {
        let tmp = std::env::temp_dir().join(format!(
            "jkcodingagent-ingest-test-{}",
            uuid::Uuid::new_v4()
        ));
        let page_path = tmp.join("wiki/concepts/test-page.md");
        let content = "# My Page\n\nSome body text.";
        let result = prepare_page_content(content, "source.pdf", &page_path).unwrap();
        assert!(result.starts_with("---"));
        assert!(result.contains("title:"));
        assert!(result.contains("sources:"));
        assert!(result.contains("My Page"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn prepare_page_content_preserves_existing_frontmatter() {
        let tmp = std::env::temp_dir().join(format!(
            "jkcodingagent-ingest-test-{}",
            uuid::Uuid::new_v4()
        ));
        let page_path = tmp.join("wiki/concepts/existing.md");
        let content = "---\ntype: entity\ntitle: Existing\n---\n\n# Existing\nBody.";
        let result = prepare_page_content(content, "source.pdf", &page_path).unwrap();
        assert!(result.contains("type: entity"));
        assert!(result.contains("title: Existing"));
        assert!(result.contains("source.pdf"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn prepare_page_content_adds_source_to_frontmatter_without_sources() {
        let tmp = std::env::temp_dir().join(format!(
            "jkcodingagent-ingest-test-{}",
            uuid::Uuid::new_v4()
        ));
        let page_path = tmp.join("wiki/concepts/nosrc.md");
        let content = "---\ntype: concept\ntitle: No Source\n---\n\nBody.";
        let result = prepare_page_content(content, "my-file.pdf", &page_path).unwrap();
        assert!(result.contains("my-file.pdf"));
        assert!(result.contains("sources:"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn analysis_system_prompt_contains_file_block_instruction() {
        let prompt = analysis_system_prompt();
        assert!(prompt.contains("FILE"));
        assert!(prompt.contains("wiki/"));
    }

    #[test]
    fn build_ingest_prompt_includes_source_name() {
        let prompt = build_ingest_prompt("test.pdf", "Some content here");
        assert!(prompt.contains("test.pdf"));
        assert!(prompt.contains("Some content here"));
    }

    #[test]
    fn merge_prompt_includes_both_versions() {
        let prompt = merge_prompt("existing text", "new text", "source.md");
        assert!(prompt.contains("existing text"));
        assert!(prompt.contains("new text"));
        assert!(prompt.contains("source.md"));
    }

    #[test]
    fn hash_file_deterministic() {
        let tmp = std::env::temp_dir().join(format!(
            "jkcodingagent-ingest-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("test.txt");
        std::fs::write(&file, "Hello World").unwrap();

        let hash1 = hash_file(&file).unwrap();
        let hash2 = hash_file(&file).unwrap();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hex
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn hash_file_different_content() {
        let tmp = std::env::temp_dir().join(format!(
            "jkcodingagent-ingest-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let file1 = tmp.join("a.txt");
        let file2 = tmp.join("b.txt");
        std::fs::write(&file1, "Content A").unwrap();
        std::fs::write(&file2, "Content B").unwrap();

        let hash1 = hash_file(&file1).unwrap();
        let hash2 = hash_file(&file2).unwrap();
        assert_ne!(hash1, hash2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn parse_file_blocks_preserves_multiline_content() {
        let output = "FILE: wiki/test.md\n# Title\n\nParagraph 1\n\nParagraph 2\n";
        let blocks = parse_file_blocks(output).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("Paragraph 1"));
        assert!(blocks[0].content.contains("Paragraph 2"));
    }

    #[test]
    fn sanitize_markdown_page_bare_code_fence() {
        let content = "```\n# Bare fence\n```";
        let sanitized = sanitize_markdown_page(content);
        assert!(!sanitized.starts_with("```"));
        assert!(sanitized.contains("# Bare fence"));
    }
}
