use std::fs;
use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use serde_json;

use super::types::{KnowledgeModelConfig, KnowledgeSettings};
use super::utils::{spawn_blocking_string, truncate_chars};

const SETTINGS_FILE: &str = "settings.json";
const MODEL_TEST_TIMEOUT_SECS: u64 = 30;

pub(crate) fn settings_path() -> Result<std::path::PathBuf> {
    Ok(super::collection::knowledge_root()?.join(SETTINGS_FILE))
}

pub(crate) fn load_settings() -> Result<KnowledgeSettings> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(KnowledgeSettings::default());
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
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
        crate::project::atomic_write(&path, &raw).map_err(|error| anyhow::anyhow!(error))?;
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
            validate_model_config("Embedding 模型", &settings.embedding_model, false)?;
            let vector = test_with_timeout(
                "Embedding 模型",
                super::embed::fetch_embedding("ping", &settings.embedding_model),
            )
            .await?;
            Ok(format!("embedding ok，维度 {}", vector.len()))
        }
        "text" => {
            validate_model_config("文本模型", &settings.text_model, true)?;
            let content = test_with_timeout(
                "文本模型",
                super::embed::call_text_model(
                    &settings.text_model,
                    "你是一个连通性测试助手，只输出 pong。",
                    "ping",
                ),
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
            validate_model_config("多模态模型", model, true)?;
            let content = test_with_timeout(
                "多模态模型",
                super::embed::call_text_model(model, "只输出 pong。", "ping"),
            )
            .await?;
            Ok(format!(
                "vision endpoint ok：{}",
                truncate_chars(content.trim(), 120)
            ))
        }
        _ => Err(format!("未知模型类型：{kind}")),
    }
}

fn validate_model_config(
    label: &str,
    model: &KnowledgeModelConfig,
    require_api_key: bool,
) -> Result<(), String> {
    if model.url.trim().is_empty() {
        return Err(format!("{label} URL 未配置"));
    }
    if model.model.trim().is_empty() {
        return Err(format!("{label} Model 未配置"));
    }
    if require_api_key && model.api_key.trim().is_empty() {
        return Err(format!("{label} API Key 未配置"));
    }
    Ok(())
}

async fn test_with_timeout<T, F>(label: &str, future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    tokio::time::timeout(Duration::from_secs(MODEL_TEST_TIMEOUT_SECS), future)
        .await
        .map_err(|_| format!("{label}测试超时（>{MODEL_TEST_TIMEOUT_SECS}s）"))?
}
