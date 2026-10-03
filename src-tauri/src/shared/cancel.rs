//! watch 通道取消信号的共享原语：等待与判定的唯一实现。
//!
//! 历史上集成域有五份语义互斥的本地副本（ssh 命令、ssh 同步、MCP 调用、
//! Python 运行器、聊天图片），其中「sender 掉线是否视为取消」各说各话。
//! 本模块把等待收敛为单一实现，并把「掉线」显式暴露为独立终态
//! （[`CancelWaitOutcome::SourceDropped`]），由调用点声明各自的策略：
//! - 掉线=取消：直接使用 [`wait_for_cancel`] 的终态或
//!   [`cancel_requested_or_dropped`] 的即时判定；
//! - 掉线=不取消（发送方任务结束不等于要求取消）：使用
//!   [`wait_for_explicit_cancel`]，掉线后转入挂起，等价于无取消源。

use tokio::sync::watch;

/// 取消等待的终态：显式区分「信号置位」与「取消源掉线」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelWaitOutcome {
    /// 取消信号已置位（watch 值为 true）。
    Cancelled,
    /// 取消源（watch sender）已掉线且从未置位。
    SourceDropped,
}

/// 异步等待取消：信号置位或取消源掉线即完成。
///
/// `None`（无取消源）永不完成，便于直接作为 `tokio::select!` 分支使用。
pub(crate) async fn wait_for_cancel(
    cancel: Option<watch::Receiver<bool>>,
) -> CancelWaitOutcome {
    let Some(mut rx) = cancel else {
        std::future::pending::<()>().await;
        unreachable!()
    };
    loop {
        // 先查当前值：已置位时不再等待下一次变更。
        if *rx.borrow() {
            return CancelWaitOutcome::Cancelled;
        }
        if rx.changed().await.is_err() {
            return CancelWaitOutcome::SourceDropped;
        }
    }
}

/// 掉线不取消策略：仅真实的信号置位触发完成；取消源掉线后转入挂起，
/// 语义等价于无取消源（`None`）。
pub(crate) async fn wait_for_explicit_cancel(cancel: Option<watch::Receiver<bool>>) {
    if let CancelWaitOutcome::SourceDropped = wait_for_cancel(cancel).await {
        std::future::pending::<()>().await;
    }
}

/// 同步判定（掉线=取消策略的即时探测）：信号已置位，或取消源已掉线。
pub(crate) fn cancel_requested_or_dropped(cancel: Option<&watch::Receiver<bool>>) -> bool {
    cancel.is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn channel(initial: bool) -> (watch::Sender<bool>, watch::Receiver<bool>) {
        watch::channel(initial)
    }

    #[tokio::test]
    async fn wait_resolves_cancelled_when_signal_set() {
        let (tx, rx) = channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).unwrap();
        });
        assert_eq!(
            wait_for_cancel(Some(rx)).await,
            CancelWaitOutcome::Cancelled
        );
    }

    #[tokio::test]
    async fn wait_resolves_immediately_when_already_set() {
        let (_tx, rx) = channel(true);
        assert_eq!(
            wait_for_cancel(Some(rx)).await,
            CancelWaitOutcome::Cancelled
        );
    }

    #[tokio::test]
    async fn wait_resolves_source_dropped_when_sender_falls() {
        let (tx, rx) = channel(false);
        drop(tx);
        assert_eq!(
            wait_for_cancel(Some(rx)).await,
            CancelWaitOutcome::SourceDropped
        );
    }

    #[tokio::test]
    async fn wait_without_source_never_resolves() {
        let settled =
            tokio::time::timeout(Duration::from_millis(50), wait_for_cancel(None)).await;
        assert!(settled.is_err());
    }

    #[tokio::test]
    async fn explicit_cancel_ignores_dropped_source() {
        let (tx, rx) = channel(false);
        drop(tx);
        let settled = tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_explicit_cancel(Some(rx)),
        )
        .await;
        assert!(settled.is_err());
    }

    #[tokio::test]
    async fn explicit_cancel_resolves_on_signal() {
        let (tx, rx) = channel(false);
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_millis(50), wait_for_explicit_cancel(Some(rx)))
            .await
            .expect("explicit cancel resolves on true");
    }

    #[test]
    fn sync_predicate_covers_set_and_dropped() {
        assert!(!cancel_requested_or_dropped(None));
        let (tx, rx) = channel(false);
        assert!(!cancel_requested_or_dropped(Some(&rx)));
        tx.send(true).unwrap();
        assert!(cancel_requested_or_dropped(Some(&rx)));
        drop(tx);
        assert!(cancel_requested_or_dropped(Some(&rx)));
    }
}
