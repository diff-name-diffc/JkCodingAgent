//! 子 Agent 模型配置与父级取消转发。
use super::*;

/// 解析子智能体模型槽位：继承父级凭据/网关（仅换模型名）或使用独立配置；
/// 输出预算与温度取子智能体自身配置。
pub(super) fn resolve_sub_agent_spec(
    config: &SubAgentConfig,
    parent: &PurposeModelSpec,
) -> PurposeModelSpec {
    let mut spec = parent.clone();
    let model_config = &config.model_config;
    if !model_config.inherit_from_parent {
        if let Some(api_base) = model_config.api_base.as_deref() {
            spec.api_base = api_base.to_string();
        }
        if let Some(api_key) = model_config.api_key.as_deref() {
            spec.api_key = api_key.to_string();
        }
    }
    if let Some(model) = model_config
        .model_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        spec.model = model.to_string();
    }
    spec.max_tokens = Some(u64::from(config.max_output_tokens));
    spec.temperature = config.temperature;
    spec
}

/// 转发取消：父取消或整体超时 → 翻转 run 级取消通道（协作式收敛）。
pub(super) async fn forward_cancellation(
    parent: Option<watch::Receiver<bool>>,
    tx: watch::Sender<bool>,
    overall_timeout: Duration,
) {
    let deadline = tokio::time::sleep(overall_timeout);
    tokio::pin!(deadline);
    match parent {
        Some(mut parent) => loop {
            tokio::select! {
                _ = &mut deadline => {
                    let _ = tx.send(true);
                    return;
                }
                changed = parent.changed() => {
                    match changed {
                        Ok(()) if *parent.borrow() => {
                            let _ = tx.send(true);
                            return;
                        }
                        Ok(()) => {}
                        Err(_) => {
                            let _ = tx.send(true);
                            return;
                        }
                    }
                }
            }
        },
        None => {
            deadline.await;
            let _ = tx.send(true);
        }
    }
}
