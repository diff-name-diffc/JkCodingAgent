use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::model::{
    validate_snapshot_id, CurrentPointer, GenerationManifest, SnapshotFile, UpdateRecord,
    SNAPSHOT_SCHEMA_VERSION,
};
use super::query::LoadedGeneration;

const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct SnapshotStore {
    session_root: PathBuf,
    workspace_id: String,
}

impl SnapshotStore {
    pub(crate) fn new(session_root: PathBuf, workspace_id: String) -> Self {
        Self {
            session_root,
            workspace_id,
        }
    }

    pub(crate) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub(crate) fn snapshots_root(&self) -> PathBuf {
        self.session_root.join("log-snapshots")
    }

    pub(crate) fn snapshot_root(&self, snapshot_id: &str) -> Result<PathBuf, String> {
        validate_snapshot_id(snapshot_id)?;
        Ok(self.snapshots_root().join(snapshot_id))
    }

    pub(crate) fn staging_root(&self, snapshot_id: &str, job_id: &str) -> Result<PathBuf, String> {
        validate_snapshot_id(job_id)?;
        Ok(self
            .snapshot_root(snapshot_id)?
            .join(".staging")
            .join(job_id))
    }

    pub(crate) fn generation_root(
        &self,
        snapshot_id: &str,
        generation: u64,
    ) -> Result<PathBuf, String> {
        if generation == 0 {
            return Err("generation 必须大于 0".into());
        }
        Ok(self
            .snapshot_root(snapshot_id)?
            .join("generations")
            .join(generation.to_string()))
    }

    pub(crate) fn prepare_staging(
        &self,
        snapshot_id: &str,
        job_id: &str,
    ) -> Result<PathBuf, String> {
        let parent = self.snapshot_root(snapshot_id)?.join(".staging");
        if parent.exists() {
            reject_symlink(&parent)?;
            std::fs::remove_dir_all(&parent).map_err(|error| {
                format!(
                    "回收上次中断的日志暂存目录失败 {}：{error}",
                    parent.display()
                )
            })?;
        }
        let root = self.staging_root(snapshot_id, job_id)?;
        create_dir(&root.join("raw"))?;
        Ok(root)
    }

    pub(crate) fn cleanup_staging(&self, snapshot_id: &str, job_id: &str) -> Result<(), String> {
        let root = self.staging_root(snapshot_id, job_id)?;
        if !root.exists() {
            return Ok(());
        }
        reject_symlink(&root)?;
        std::fs::remove_dir_all(&root)
            .map_err(|error| format!("清理日志暂存目录失败 {}：{error}", root.display()))
    }

    pub(crate) fn load_update(&self, snapshot_id: &str) -> Result<Option<UpdateRecord>, String> {
        let path = self.snapshot_root(snapshot_id)?.join("update.json");
        read_optional_json(&path)
    }

    pub(crate) fn write_update(&self, record: &UpdateRecord) -> Result<(), String> {
        let path = self.snapshot_root(&record.snapshot_id)?.join("update.json");
        write_json_atomic(&path, record)
    }

    pub(crate) fn load_current(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<GenerationManifest>, String> {
        Ok(self
            .load_current_generation(snapshot_id)?
            .map(|value| value.manifest))
    }

    pub(crate) fn load_current_generation(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<LoadedGeneration>, String> {
        let pointer_path = self.snapshot_root(snapshot_id)?.join("current.json");
        let Some(pointer): Option<CurrentPointer> = read_optional_json(&pointer_path)? else {
            return Ok(None);
        };
        if pointer.schema_version != SNAPSHOT_SCHEMA_VERSION
            || pointer.snapshot_id != snapshot_id
            || pointer.workspace_id != self.workspace_id
        {
            return Err("日志快照 current.json 的会话或版本不匹配".into());
        }
        let root = self.generation_root(snapshot_id, pointer.generation)?;
        let manifest: GenerationManifest = read_json(&root.join("manifest.json"))?;
        validate_manifest(
            &manifest,
            snapshot_id,
            &self.workspace_id,
            pointer.generation,
        )?;
        Ok(Some(LoadedGeneration { manifest, root }))
    }

    pub(crate) fn write_generation_manifest(
        &self,
        staging: &Path,
        manifest: &GenerationManifest,
    ) -> Result<(), String> {
        write_json_atomic(&staging.join("manifest.json"), manifest)
    }

    pub(crate) fn publish(
        &self,
        snapshot_id: &str,
        job_id: &str,
        generation: u64,
        old_generation: Option<u64>,
    ) -> Result<(), String> {
        let staging = self.staging_root(snapshot_id, job_id)?;
        let target = self.generation_root(snapshot_id, generation)?;
        if target.exists() {
            reject_symlink(&target)?;
            std::fs::remove_dir_all(&target).map_err(|error| {
                format!("回收未发布日志代次目录失败 {}：{error}", target.display())
            })?;
        }
        create_dir(
            target
                .parent()
                .ok_or_else(|| "代次目录缺少父目录".to_string())?,
        )?;
        std::fs::rename(&staging, &target).map_err(|error| {
            format!(
                "发布日志代次失败：{} → {}：{error}",
                staging.display(),
                target.display()
            )
        })?;
        let pointer = CurrentPointer {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            snapshot_id: snapshot_id.to_string(),
            workspace_id: self.workspace_id.clone(),
            generation,
        };
        if let Err(error) = write_json_atomic(
            &self.snapshot_root(snapshot_id)?.join("current.json"),
            &pointer,
        ) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(error);
        }
        if let Some(old) = old_generation.filter(|old| *old != generation) {
            if let Ok(old_root) = self.generation_root(snapshot_id, old) {
                if old_root.exists() {
                    if let Err(error) = std::fs::remove_dir_all(&old_root) {
                        eprintln!(
                            "[log-snapshot] 旧代次回收失败 {}：{error}",
                            old_root.display()
                        );
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn raw_file_path(
        &self,
        generation: &LoadedGeneration,
        file_id: u64,
    ) -> Result<(SnapshotFile, PathBuf), String> {
        let file = generation
            .manifest
            .files
            .iter()
            .find(|file| file.file_id == file_id)
            .cloned()
            .ok_or_else(|| format!("file_id {file_id} 不在当前日志代次中"))?;
        let expected = format!("raw/{file_id}.log");
        if file.raw_path != expected {
            return Err("日志清单中的 raw_path 不合法".into());
        }
        let path = generation.root.join(&file.raw_path);
        reject_symlink(&path)?;
        Ok((file, path))
    }

    pub(crate) fn list_snapshot_ids(&self) -> Result<Vec<String>, String> {
        let root = self.snapshots_root();
        if !root.exists() {
            return Ok(Vec::new());
        }
        reject_symlink(&root)?;
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|error| format!("列举日志快照失败 {}：{error}", root.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if validate_snapshot_id(&id).is_ok() {
                ids.push(id);
            }
            if ids.len() >= 100 {
                break;
            }
        }
        ids.sort();
        Ok(ids)
    }
}

fn validate_manifest(
    manifest: &GenerationManifest,
    snapshot_id: &str,
    workspace_id: &str,
    generation: u64,
) -> Result<(), String> {
    if manifest.schema_version != SNAPSHOT_SCHEMA_VERSION
        || manifest.snapshot_id != snapshot_id
        || manifest.workspace_id != workspace_id
        || manifest.generation != generation
        || manifest.index_backend != "fts5-trigram"
    {
        return Err("日志代次清单的版本、会话或索引格式不匹配".into());
    }
    // raw_path 是 join 的输入：形态不合法（绝对路径 / ../ 逃逸）会让 scan 的
    // ripgrep 与索引读取代次目录之外的文件，read 路径的检查在这里统一前置。
    for file in &manifest.files {
        if file.raw_path != format!("raw/{}.log", file.file_id) || file.file_id == 0 {
            return Err("日志清单中的 raw_path 不合法".into());
        }
    }
    Ok(())
}

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    reject_symlink(path)?;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("读取日志元数据失败 {}：{error}", path.display()))?;
    if metadata.len() > MAX_JSON_BYTES {
        return Err(format!("日志元数据文件过大：{}", path.display()));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("打开日志元数据失败 {}：{error}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取日志元数据失败 {}：{error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("日志元数据格式无效：{error}"))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "日志元数据缺少父目录".to_string())?;
    create_dir(parent)?;
    reject_symlink(parent)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("日志元数据超过大小上限".into());
    }
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        uuid::Uuid::new_v4(),
        std::process::id()
    ));
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("创建临时元数据失败：{error}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("写入临时元数据失败：{error}"))?;
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("原子替换日志元数据失败 {}：{error}", path.display())
    })
}

fn create_dir(path: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| format!("创建日志目录失败 {}：{error}", path.display()))
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("检查日志路径失败 {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("日志路径不允许符号链接：{}", path.display()));
    }
    Ok(())
}
