//! Worker 的 owned 结果整理服务，不共享 UsageTracker，也不写聊天消息。
use super::*;
use futures::future::BoxFuture;
use rig::{completion::CompletionModel, message::ToolCall};
use std::sync::Arc;

pub(crate) struct PreparedCompletion {
    pub display: String,
    pub context: String,
    pub mode: String,
    pub usage: Option<crate::agent::db::LlmUsage>,
}

pub(crate) type ResultPreparer = Arc<
    dyn Fn(ToolCall, String, RigToolResultPolicy) -> BoxFuture<'static, PreparedCompletion>
        + Send
        + Sync,
>;

pub(crate) fn owned_preparer<M: CompletionModel + Clone + 'static>(
    summary: Option<&RigSummaryModel<'_, M>>,
    user_question: Option<String>,
) -> ResultPreparer {
    let model = summary.map(|s| (s.model.clone(), s.max_tokens, s.temperature));
    Arc::new(move |call, raw, policy| {
        let model = model.clone();
        let question = user_question.clone();
        Box::pin(async move {
            let prepared = prepare_rig_tool_result(
                &call.function.name,
                &call.function.arguments,
                &raw,
                &policy,
            );
            if !prepared.needs_summary {
                return PreparedCompletion {
                    display: prepared.display_content,
                    context: prepared.context_payload,
                    mode: prepared.result_mode.into(),
                    usage: None,
                };
            }
            if let Some((model, max_tokens, temperature)) = model {
                let summary = RigSummaryModel {
                    model: &model,
                    max_tokens,
                    temperature,
                };
                match summarize_with_rig_model(
                    &summary,
                    &call.function.name,
                    &raw,
                    question.as_deref(),
                    prepared.compress_intent.as_deref(),
                )
                .await
                {
                    Ok((result, usage)) => {
                        return PreparedCompletion {
                            display: bound_inline_tool_result(result.display_content),
                            context: bound_inline_tool_result(result.context_payload),
                            mode: summary_result_mode(prepared.compress_intent.is_some()).into(),
                            usage: usage.has_values().then(|| llm_usage_from_rig(&usage)),
                        }
                    }
                    Err(error) => eprintln!("工具摘要失败，使用结构化提取：{error:#}"),
                }
                let text =
                    bound_inline_tool_result(extract_structured_summary(&call.function.name, &raw));
                return PreparedCompletion {
                    display: text.clone(),
                    context: text,
                    mode: "structured_fallback".into(),
                    usage: None,
                };
            }
            let text = truncate_tool_result(
                &raw,
                raw.chars().count(),
                inline_max_chars(&call.function.name, &call.function.arguments),
                ARTIFACT_REMAINDER_NOTE,
            );
            PreparedCompletion {
                display: text.clone(),
                context: text,
                mode: "truncated".into(),
                usage: None,
            }
        })
    })
}

pub(crate) fn raw_preparer() -> ResultPreparer {
    Arc::new(|call, raw, mut policy| {
        Box::pin(async move {
            policy.default_compress = false;
            let mut arguments = call.function.arguments;
            if let Some(args) = arguments.as_object_mut() {
                args.insert("compress".into(), false.into());
            }
            let prepared = prepare_rig_tool_result(&call.function.name, &arguments, &raw, &policy);
            PreparedCompletion {
                display: prepared.display_content,
                context: prepared.context_payload,
                mode: prepared.result_mode.into(),
                usage: None,
            }
        })
    })
}
