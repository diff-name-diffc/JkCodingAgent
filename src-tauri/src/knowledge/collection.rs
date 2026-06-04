use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::types::KnowledgeCollection;
use super::utils::{normalize_path_string, now_ms, spawn_blocking_string};

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

pub(crate) fn knowledge_root() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("找不到用户主目录"))?;
    Ok(home.join(".jkcodingagent").join("knowledge"))
}

fn collections_root() -> Result<PathBuf> {
    Ok(knowledge_root()?.join("collections"))
}

fn collections_path() -> Result<PathBuf> {
    Ok(knowledge_root()?.join(super::COLLECTIONS_FILE))
}

pub(crate) fn collection_root_checked(collection: &KnowledgeCollection) -> Result<PathBuf> {
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

pub(crate) fn load_collections() -> Result<Vec<KnowledgeCollection>> {
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
    crate::project::atomic_write(&path, &serde_json::to_string_pretty(collections)?)
        .map_err(|error| anyhow!(error))
}

pub(crate) fn find_collection(collection_id: &str) -> Result<KnowledgeCollection> {
    load_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or_else(|| anyhow!("知识库集合不存在：{collection_id}"))
        .and_then(|collection| {
            collection_root_checked(&collection)?;
            Ok(collection)
        })
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
        crate::project::atomic_write(
            &overview,
            "---\ntype: overview\ntitle: Overview\n---\n\n# Overview\n",
        )
        .map_err(|error| anyhow!(error))?;
    }
    Ok(())
}

pub(crate) fn touch_collection(collection_id: &str) -> Result<()> {
    let mut collections = load_collections()?;
    let Some(collection) = collections.iter_mut().find(|item| item.id == collection_id) else {
        return Err(anyhow!("知识库集合不存在：{collection_id}"));
    };
    collection.updated_at = now_ms();
    save_collections(&collections)
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
        super::jobs::cancel_and_remove_jobs_for_collection(&collection_id)?;
        let root = collection_root_checked(&collection)?;
        if root.exists() {
            fs::remove_dir_all(&root)
                .with_context(|| format!("删除集合目录失败：{}", root.display()))?;
        }
        save_collections(&collections)
    })
    .await
}
