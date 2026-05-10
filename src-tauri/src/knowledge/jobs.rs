
use anyhow::{anyhow, Result};

use super::collection::find_collection;
use super::types::KnowledgeIngestJob;
use super::utils::{now_ms, spawn_blocking_string};

const INGEST_JOBS_FILE: &str = "ingest-jobs.json";

fn load_ingest_jobs() -> Result<Vec<KnowledgeIngestJob>> {
    let path = super::collection::knowledge_root()?.join(INGEST_JOBS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn save_ingest_jobs(jobs: &[KnowledgeIngestJob]) -> Result<()> {
    let path = super::collection::knowledge_root()?.join(INGEST_JOBS_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::project::atomic_write(&path, &serde_json::to_string_pretty(jobs)?)
        .map_err(|error| anyhow!(error))
}

pub(crate) fn upsert_ingest_job(job: KnowledgeIngestJob) -> Result<()> {
    let mut jobs = load_ingest_jobs()?;
    if let Some(existing) = jobs.iter_mut().find(|item| item.id == job.id) {
        *existing = job;
    } else {
        jobs.push(job);
    }
    save_ingest_jobs(&jobs)
}

pub(crate) fn update_job_status(
    job_id: &str,
    status: &str,
    message: &str,
) -> Result<KnowledgeIngestJob> {
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
    super::utils::set_cancel_token(&job_id);
    spawn_blocking_string(move || update_job_status(&job_id, "cancelled", "任务已取消")).await
}

#[tauri::command]
pub async fn knowledge_retry_ingest(job_id: String) -> Result<KnowledgeIngestJob, String> {
    let (job, settings) = spawn_blocking_string({
        let job_id = job_id.clone();
        move || {
            let settings = super::settings::load_settings()?;
            let job = load_ingest_jobs()?
                .into_iter()
                .find(|j| j.id == job_id)
                .ok_or_else(|| anyhow!("导入任务不存在：{job_id}"))?;
            Ok((job, settings))
        }
    })
    .await?;

    let collection = find_collection(&job.collection_id).map_err(|e| e.to_string())?;
    update_job_status(&job_id, "running", "正在重试").map_err(|e| e.to_string())?;
    super::ingest::ingest_one_source_with_job(
        collection,
        job.source_path,
        settings,
        job_id,
    )
    .await
}