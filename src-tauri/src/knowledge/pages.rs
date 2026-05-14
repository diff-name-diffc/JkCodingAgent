use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::collection::{collection_root_checked, find_collection, touch_collection};
use super::types::{KnowledgePageContent, KnowledgePageSummary, PageMeta};
use super::utils::{normalize_path_string, spawn_blocking_string, title_from_slug};

pub(crate) fn relative_to_collection(
    collection: &super::types::KnowledgeCollection,
    path: &Path,
) -> Result<String> {
    let root = collection_root_checked(collection)?;
    let rel = path.strip_prefix(root)?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

fn resolve_collection_relative_path(
    collection: &super::types::KnowledgeCollection,
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

fn ensure_wiki_markdown_path(
    collection: &super::types::KnowledgeCollection,
    path: &Path,
) -> Result<()> {
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

fn read_first_kb(path: &Path, kb: usize) -> String {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let mut buf = vec![0u8; kb * 1024];
    let n = file.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    String::from_utf8(buf).unwrap_or_default()
}

fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

pub(crate) fn strip_frontmatter(content: &str) -> &str {
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

pub(crate) fn extract_frontmatter_array(content: &str, key: &str) -> BTreeSet<String> {
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

fn normalize_source_name(name: String) -> String {
    let unescaped = name.replace("\\\"", "\"").replace("\\\\", "\\");
    Path::new(&unescaped)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&unescaped)
        .to_string()
}

pub(crate) fn parse_page_meta(content: &str, path: &Path) -> PageMeta {
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

pub(crate) fn list_pages_inner(
    collection: &super::types::KnowledgeCollection,
) -> Result<Vec<KnowledgePageSummary>> {
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
        let partial = read_first_kb(&path, 3);
        let meta = parse_page_meta(&partial, &path);
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

fn read_page_for_agent_inner(
    collection_id: &str,
    relative_path: &str,
    max_chars: usize,
) -> Result<String> {
    let collection = find_collection(collection_id)?;
    let page_path = resolve_collection_relative_path(&collection, relative_path)?;
    ensure_wiki_markdown_path(&collection, &page_path)?;
    let content = fs::read_to_string(&page_path)?;
    let trimmed = super::utils::truncate_chars(&content, max_chars);
    Ok(format!(
        "[{}] {}\npath: {}\n\n{}",
        collection.name,
        parse_page_meta(&content, &page_path).title,
        relative_to_collection(&collection, &page_path)?,
        trimmed
    ))
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
        crate::project::atomic_write(&page_path, &content).map_err(|error| anyhow!(error))?;
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
    // NOTE: 在文件删除与向量库删除之间存在已知竞争条件——在此期间，其他操作可能重新读取该页面。
    // 这是最终一致性设计固有的，在并发删除同一页面时无额外副作用。
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
    super::vector_store::delete_page(&root, &page_path)
        .await
        .map_err(|error| error.to_string())
}
