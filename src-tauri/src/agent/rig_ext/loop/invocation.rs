//! 单次调用的不可变身份；task-local 只用于 rig 回调边界。

use serde_json::Value;
use std::future::Future;
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub(crate) struct ToolInvocationContext {
    pub workspace_id: String,
    pub agent_run_id: String,
    pub task_id: String,
    pub tool_call_id: String,
    pub root_request_message_id: String,
    pub cancel_rx: watch::Receiver<bool>,
    /// enqueue 准入产出的 effective 参数（schema 默认注入 + Draft 2020-12 已校验）。
    /// 调度器路径在 enqueue 计算一次后随本结构流入 worker，`before_call`/`execute`
    /// 直接消费、不再重算；裸路径（顺序批/无上下文）为 None，由策略层自行计算。
    pub prepared_arguments: Option<Value>,
}

tokio::task_local! {
    static INVOCATION: ToolInvocationContext;
}

impl ToolInvocationContext {
    /// 回调入口取 owned clone；新 spawn 的任务必须显式传递此值。
    pub(crate) fn current() -> Option<Self> {
        INVOCATION.try_with(Clone::clone).ok()
    }

    pub(crate) async fn scope<F: Future>(self, future: F) -> F::Output {
        INVOCATION.scope(self, future).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn concurrent_invocations_keep_identity_across_awaits() {
        let (_cancel, cancel_rx) = watch::channel(false);
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut jobs = tokio::task::JoinSet::new();
        for id in ["first", "second"] {
            let context = ToolInvocationContext {
                workspace_id: "workspace".into(),
                agent_run_id: "run".into(),
                task_id: id.into(),
                tool_call_id: id.into(),
                root_request_message_id: "request".into(),
                cancel_rx: cancel_rx.clone(),
                prepared_arguments: None,
            };
            let barrier = barrier.clone();
            jobs.spawn(context.scope(async move {
                let captured = ToolInvocationContext::current().unwrap();
                barrier.wait().await;
                assert_eq!(captured.tool_call_id, id);
                assert_eq!(ToolInvocationContext::current().unwrap().tool_call_id, id);
                // Tokio spawn 不继承 task-local，避免误关联父调用。
                assert!(tokio::spawn(async { ToolInvocationContext::current() })
                    .await
                    .unwrap()
                    .is_none());
            }));
        }
        while let Some(result) = jobs.join_next().await {
            result.unwrap();
        }
        assert!(ToolInvocationContext::current().is_none());
    }
}
