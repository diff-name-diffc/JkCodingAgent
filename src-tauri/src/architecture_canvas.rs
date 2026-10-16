//! 架构画板文件管理：画板统一保存到应用资源目录
//! `~/.jkcodingagent/architecture-canvases/`，替代 Excalidraw 内置的
//! 「下载到本地目录」原生保存。
//!
//! 目录布局：
//! - `{canvas_id}.excalidraw`：标准 Excalidraw 场景 JSON
//!   （type/version/source/elements/appState/files），展示名写入
//!   `appState.name`，文件单独拷走也不丢名字；
//! - `index.json`：轻量元数据索引（id/name/createdAt/updatedAt），列表
//!   不必逐个解析可能含内联图片的大场景文件。索引以文件系统为事实源
//!   做调和：文件缺失剔条目、孤儿场景文件补条目。
//!
//! 命令只接受画板 id（白名单字符集）与展示名，不接受任何路径参数，
//! 目录穿越在 id 校验处封堵。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CANVAS_DIR_NAME: &str = "architecture-canvases";
const INDEX_FILE_NAME: &str = "index.json";
const CANVAS_FILE_EXT: &str = "excalidraw";
const INDEX_VERSION: u32 = 1;
const MAX_CANVAS_NAME_CHARS: usize = 80;
const DEFAULT_CANVAS_NAME: &str = "未命名画板";

/// 画板文件元数据（前端契约，camelCase）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectureCanvasMeta {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 画板文件完整载荷：元数据 + `.excalidraw` 场景 JSON 原文（前端契约）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectureCanvasFile {
    pub meta: ArchitectureCanvasMeta,
    pub data_json: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CanvasIndex {
    version: u32,
    #[serde(default)]
    canvases: Vec<ArchitectureCanvasMeta>,
}

fn canvases_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".jkcodingagent").join(CANVAS_DIR_NAME))
        .ok_or_else(|| "无法解析用户主目录".to_string())
}

/// 画板 id 白名单（防目录穿越的唯一闸门）：[0-9A-Za-z_-]{1,64}。
fn is_valid_canvas_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 展示名归一化：去首尾空白、空值回退默认名、限长（按字符截断）。
fn sanitize_canvas_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_CANVAS_NAME.to_string();
    }
    trimmed.chars().take(MAX_CANVAS_NAME_CHARS).collect()
}

fn canvas_file_path(dir: &Path, canvas_id: &str) -> PathBuf {
    dir.join(format!("{canvas_id}.{CANVAS_FILE_EXT}"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn file_mtime_ms(path: &Path) -> i64 {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(now_ms)
}

/// tmp + rename 原子写：避免崩溃留下半截场景/索引文件。
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("写入临时文件失败（{}）：{e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("落盘失败（{}）：{e}", path.display()))
}

fn empty_index() -> CanvasIndex {
    CanvasIndex {
        version: INDEX_VERSION,
        canvases: Vec::new(),
    }
}

/// 索引缺失/损坏均按空索引处理（文件是事实源，调和会补回）。
fn read_index(dir: &Path) -> CanvasIndex {
    let Ok(raw) = fs::read_to_string(dir.join(INDEX_FILE_NAME)) else {
        return empty_index();
    };
    serde_json::from_str(&raw).unwrap_or_else(|_| empty_index())
}

fn write_index(dir: &Path, index: &CanvasIndex) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(index).map_err(|e| format!("索引序列化失败：{e}"))?;
    atomic_write(&dir.join(INDEX_FILE_NAME), &content)
}

/// 从场景 JSON 尽力取展示名（appState.name）。
fn name_from_scene_json(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return DEFAULT_CANVAS_NAME.to_string();
    };
    value
        .get("appState")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .map(sanitize_canvas_name)
        .unwrap_or_else(|| DEFAULT_CANVAS_NAME.to_string())
}

/// 把展示名注入场景 JSON 的 appState.name；data 必须是 JSON 对象。
fn inject_scene_name(data_json: &str, name: &str) -> Result<String, String> {
    let mut data: Value =
        serde_json::from_str(data_json).map_err(|e| format!("画板数据不是合法 JSON：{e}"))?;
    let Some(obj) = data.as_object_mut() else {
        return Err("画板数据必须是 JSON 对象".to_string());
    };
    let app_state = obj
        .entry("appState".to_string())
        .or_insert_with(|| json!({}));
    if !app_state.is_object() {
        *app_state = json!({});
    }
    if let Some(app_state) = app_state.as_object_mut() {
        app_state.insert("name".to_string(), Value::String(name.to_string()));
    }
    serde_json::to_string_pretty(&data).map_err(|e| format!("画板数据序列化失败：{e}"))
}

/// 调和索引与文件系统：剔除文件缺失的条目，为孤儿场景文件补条目。
/// 返回是否发生变化（变化需回写索引）。
fn reconcile_index(dir: &Path, index: &mut CanvasIndex) -> bool {
    let mut changed = false;
    index.canvases.retain(|meta| {
        let keep = canvas_file_path(dir, &meta.id).is_file();
        if !keep {
            changed = true;
        }
        keep
    });
    let mut known: HashSet<String> = index.canvases.iter().map(|m| m.id.clone()).collect();
    let Ok(entries) = fs::read_dir(dir) else {
        return changed;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(id) = file_name.strip_suffix(&format!(".{CANVAS_FILE_EXT}")) else {
            continue;
        };
        if !is_valid_canvas_id(id) || known.contains(id) {
            continue;
        }
        let ts = file_mtime_ms(&entry.path());
        let name = fs::read_to_string(entry.path())
            .map(|raw| name_from_scene_json(&raw))
            .unwrap_or_else(|_| DEFAULT_CANVAS_NAME.to_string());
        index.canvases.push(ArchitectureCanvasMeta {
            id: id.to_string(),
            name,
            created_at: ts,
            updated_at: ts,
        });
        known.insert(id.to_string());
        changed = true;
    }
    changed
}

fn list_in_dir(dir: &Path) -> Result<Vec<ArchitectureCanvasMeta>, String> {
    let mut index = read_index(dir);
    if reconcile_index(dir, &mut index) {
        write_index(dir, &index)?;
    }
    index
        .canvases
        .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(index.canvases)
}

fn load_in_dir(dir: &Path, canvas_id: &str) -> Result<ArchitectureCanvasFile, String> {
    if !is_valid_canvas_id(canvas_id) {
        return Err(format!("非法的画板标识：{canvas_id}"));
    }
    let path = canvas_file_path(dir, canvas_id);
    let data_json =
        fs::read_to_string(&path).map_err(|e| format!("读取画板文件失败（{canvas_id}）：{e}"))?;
    let mut index = read_index(dir);
    let meta = match index.canvases.iter().find(|m| m.id == canvas_id) {
        Some(meta) => meta.clone(),
        None => {
            // 索引缺失（如手工拷入）：补录后返回，索引回写 best-effort。
            let ts = file_mtime_ms(&path);
            let meta = ArchitectureCanvasMeta {
                id: canvas_id.to_string(),
                name: name_from_scene_json(&data_json),
                created_at: ts,
                updated_at: ts,
            };
            index.canvases.push(meta.clone());
            let _ = write_index(dir, &index);
            meta
        }
    };
    Ok(ArchitectureCanvasFile { meta, data_json })
}

fn save_in_dir(
    dir: &Path,
    canvas_id: Option<String>,
    name: &str,
    data_json: &str,
) -> Result<ArchitectureCanvasMeta, String> {
    let name = sanitize_canvas_name(name);
    let id = match canvas_id {
        Some(id) => {
            if !is_valid_canvas_id(&id) {
                return Err(format!("非法的画板标识：{id}"));
            }
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    let content = inject_scene_name(data_json, &name)?;
    fs::create_dir_all(dir).map_err(|e| format!("创建画板目录失败：{e}"))?;
    atomic_write(&canvas_file_path(dir, &id), &content)?;
    let now = now_ms();
    let mut index = read_index(dir);
    let meta = match index.canvases.iter_mut().find(|m| m.id == id) {
        Some(existing) => {
            existing.name = name;
            existing.updated_at = now;
            existing.clone()
        }
        None => {
            let meta = ArchitectureCanvasMeta {
                id: id.clone(),
                name,
                created_at: now,
                updated_at: now,
            };
            index.canvases.push(meta.clone());
            meta
        }
    };
    write_index(dir, &index)?;
    Ok(meta)
}

fn delete_in_dir(dir: &Path, canvas_id: &str) -> Result<bool, String> {
    if !is_valid_canvas_id(canvas_id) {
        return Err(format!("非法的画板标识：{canvas_id}"));
    }
    match fs::remove_file(canvas_file_path(dir, canvas_id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("删除画板文件失败：{e}")),
    }
    let mut index = read_index(dir);
    index.canvases.retain(|m| m.id != canvas_id);
    write_index(dir, &index)?;
    Ok(true)
}

fn rename_in_dir(
    dir: &Path,
    canvas_id: &str,
    name: &str,
) -> Result<ArchitectureCanvasMeta, String> {
    if !is_valid_canvas_id(canvas_id) {
        return Err(format!("非法的画板标识：{canvas_id}"));
    }
    let name = sanitize_canvas_name(name);
    let path = canvas_file_path(dir, canvas_id);
    let raw = fs::read_to_string(&path).map_err(|_| "画板文件不存在".to_string())?;
    // 同步文件内 appState.name；场景 JSON 损坏不阻断改名（索引仍是事实源之一）。
    if let Ok(content) = inject_scene_name(&raw, &name) {
        atomic_write(&path, &content)?;
    }
    let now = now_ms();
    let mut index = read_index(dir);
    let Some(existing) = index.canvases.iter_mut().find(|m| m.id == canvas_id) else {
        return Err("画板不存在".to_string());
    };
    existing.name = name;
    existing.updated_at = now;
    let meta = existing.clone();
    write_index(dir, &index)?;
    Ok(meta)
}

fn reveal_path_in_dir(dir: &Path, canvas_id: &str) -> Result<String, String> {
    if !is_valid_canvas_id(canvas_id) {
        return Err(format!("非法的画板标识：{canvas_id}"));
    }
    let path = canvas_file_path(dir, canvas_id);
    if !path.is_file() {
        return Err("画板文件不存在".to_string());
    }
    Ok(path.to_string_lossy().to_string())
}

/// 画板列表（按更新时间倒序）；附带索引调和。
#[tauri::command]
pub async fn architecture_canvas_list() -> Result<Vec<ArchitectureCanvasMeta>, String> {
    tokio::task::spawn_blocking(|| {
        let dir = canvases_dir()?;
        fs::create_dir_all(&dir).map_err(|e| format!("创建画板目录失败：{e}"))?;
        list_in_dir(&dir)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 读取画板文件（元数据 + 场景 JSON 原文）。
#[tauri::command]
pub async fn architecture_canvas_load(canvas_id: String) -> Result<ArchitectureCanvasFile, String> {
    tokio::task::spawn_blocking(move || load_in_dir(&canvases_dir()?, &canvas_id))
        .await
        .map_err(|e| e.to_string())?
}

/// 保存画板：canvas_id 为 None 时新建（uuid），否则覆盖既有文件。
#[tauri::command]
pub async fn architecture_canvas_save(
    canvas_id: Option<String>,
    name: String,
    data_json: String,
) -> Result<ArchitectureCanvasMeta, String> {
    tokio::task::spawn_blocking(move || save_in_dir(&canvases_dir()?, canvas_id, &name, &data_json))
        .await
        .map_err(|e| e.to_string())?
}

/// 删除画板文件与索引条目；幂等（文件已缺失也返回 true）。
#[tauri::command]
pub async fn architecture_canvas_delete(canvas_id: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || delete_in_dir(&canvases_dir()?, &canvas_id))
        .await
        .map_err(|e| e.to_string())?
}

/// 重命名画板：同步索引与文件内 appState.name。
#[tauri::command]
pub async fn architecture_canvas_rename(
    canvas_id: String,
    name: String,
) -> Result<ArchitectureCanvasMeta, String> {
    tokio::task::spawn_blocking(move || rename_in_dir(&canvases_dir()?, &canvas_id, &name))
        .await
        .map_err(|e| e.to_string())?
}

/// 返回画板文件绝对路径（前端「在文件夹中显示」用，不读内容）。
#[tauri::command]
pub async fn architecture_canvas_reveal_path(canvas_id: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || reveal_path_in_dir(&canvases_dir()?, &canvas_id))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDirGuard;

    fn scene_json(name: &str) -> String {
        json!({
            "type": "excalidraw",
            "version": 2,
            "source": "test",
            "elements": [],
            "appState": { "name": name },
            "files": {},
        })
        .to_string()
    }

    #[test]
    fn canvas_id_whitelist() {
        assert!(is_valid_canvas_id("abc-DEF_123"));
        assert!(is_valid_canvas_id(&uuid::Uuid::new_v4().to_string()));
        assert!(!is_valid_canvas_id(""));
        assert!(!is_valid_canvas_id("../escape"));
        assert!(!is_valid_canvas_id("a/b"));
        assert!(!is_valid_canvas_id("a b"));
        assert!(!is_valid_canvas_id(&"x".repeat(65)));
    }

    #[test]
    fn canvas_name_sanitized() {
        assert_eq!(sanitize_canvas_name("  我的画板  "), "我的画板");
        assert_eq!(sanitize_canvas_name("   "), DEFAULT_CANVAS_NAME);
        assert_eq!(sanitize_canvas_name(""), DEFAULT_CANVAS_NAME);
        let long = "长".repeat(120);
        assert_eq!(
            sanitize_canvas_name(&long).chars().count(),
            MAX_CANVAS_NAME_CHARS
        );
    }

    #[test]
    fn save_then_load_round_trip() {
        let dir = TempDirGuard::new("arch-canvas");
        let meta = save_in_dir(dir.path(), None, "  系统架构 ", &scene_json("ignored")).unwrap();
        assert_eq!(meta.name, "系统架构");
        assert_eq!(meta.created_at, meta.updated_at);

        let loaded = load_in_dir(dir.path(), &meta.id).unwrap();
        assert_eq!(loaded.meta, meta);
        // 展示名以保存时的 name 为准注入 appState.name。
        assert_eq!(name_from_scene_json(&loaded.data_json), "系统架构");

        let list = list_in_dir(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, meta.id);
    }

    #[test]
    fn save_existing_updates_name_and_updated_at() {
        let dir = TempDirGuard::new("arch-canvas");
        let meta = save_in_dir(dir.path(), None, "v1", &scene_json("v1")).unwrap();
        let updated = save_in_dir(
            dir.path(),
            Some(meta.id.clone()),
            "v2",
            &scene_json("stale"),
        )
        .unwrap();
        assert_eq!(updated.id, meta.id);
        assert_eq!(updated.name, "v2");
        assert_eq!(updated.created_at, meta.created_at);
        assert!(updated.updated_at >= meta.updated_at);
        assert_eq!(list_in_dir(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn reconcile_drops_missing_and_adopts_orphans() {
        let dir = TempDirGuard::new("arch-canvas");
        let keep = save_in_dir(dir.path(), None, "保留", &scene_json("保留")).unwrap();
        let gone = save_in_dir(dir.path(), None, "被删", &scene_json("被删")).unwrap();
        fs::remove_file(canvas_file_path(dir.path(), &gone.id)).unwrap();
        // 手工拷入的孤儿文件（索引无条目）。
        fs::write(
            dir.path().join("manual-copy.excalidraw"),
            scene_json("手工拷入"),
        )
        .unwrap();

        let list = list_in_dir(dir.path()).unwrap();
        let ids: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(keep.id.as_str()));
        assert!(!ids.contains(gone.id.as_str()));
        assert!(ids.contains("manual-copy"));
        let orphan = list.iter().find(|m| m.id == "manual-copy").unwrap();
        assert_eq!(orphan.name, "手工拷入");
    }

    #[test]
    fn delete_is_idempotent_and_cleans_index() {
        let dir = TempDirGuard::new("arch-canvas");
        let meta = save_in_dir(dir.path(), None, "待删", &scene_json("待删")).unwrap();
        assert!(delete_in_dir(dir.path(), &meta.id).unwrap());
        assert!(list_in_dir(dir.path()).unwrap().is_empty());
        // 文件已缺失再删仍为成功（幂等）。
        assert!(delete_in_dir(dir.path(), &meta.id).unwrap());
    }

    #[test]
    fn rename_patches_index_and_scene_name() {
        let dir = TempDirGuard::new("arch-canvas");
        let meta = save_in_dir(dir.path(), None, "旧名", &scene_json("旧名")).unwrap();
        let renamed = rename_in_dir(dir.path(), &meta.id, "新名").unwrap();
        assert_eq!(renamed.name, "新名");
        let loaded = load_in_dir(dir.path(), &meta.id).unwrap();
        assert_eq!(name_from_scene_json(&loaded.data_json), "新名");
    }

    #[test]
    fn invalid_ids_rejected() {
        let dir = TempDirGuard::new("arch-canvas");
        assert!(save_in_dir(dir.path(), Some("../x".into()), "n", &scene_json("n")).is_err());
        assert!(load_in_dir(dir.path(), "a/b").is_err());
        assert!(delete_in_dir(dir.path(), "a b").is_err());
        assert!(rename_in_dir(dir.path(), "..", "n").is_err());
        assert!(reveal_path_in_dir(dir.path(), "../etc").is_err());
    }

    #[test]
    fn invalid_scene_json_rejected() {
        let dir = TempDirGuard::new("arch-canvas");
        assert!(save_in_dir(dir.path(), None, "n", "not json").is_err());
        assert!(save_in_dir(dir.path(), None, "n", "[1,2]").is_err());
    }
}
