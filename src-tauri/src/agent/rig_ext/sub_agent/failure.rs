//! 按工具名与声明轮次记录失败；旧轮次的迟到成功不能重置新失败。
use crate::agent::db::tool_completions::ToolCompletion;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct FailurePolicy {
    seen: HashSet<i64>,
    failures: HashMap<String, i64>,
    rounds: HashMap<(String, i64), (usize, bool)>,
}

impl FailurePolicy {
    pub fn register(&mut self, names: impl Iterator<Item = String>, round: i64) {
        for name in names {
            self.rounds.entry((name, round)).or_default().0 += 1;
        }
    }
    /// 返回首次失败的批次，以及是否应停止接纳新调用并强制最终结论。
    pub fn ingest<'a>(
        &mut self,
        events: impl Iterator<Item = &'a ToolCompletion>,
    ) -> (Vec<i64>, bool) {
        let mut failed_rounds = Vec::new();
        let mut escalate = false;
        for event in events {
            if !self.seen.insert(event.event_id) {
                continue;
            }
            let key = (event.tool_name.clone(), event.dispatch_round);
            let round = self.rounds.entry(key.clone()).or_insert((1, false));
            round.0 = round.0.saturating_sub(1);
            if event.status != "succeeded" && event.status != "cancelled" {
                round.1 = true;
                if let Some(previous) = self.failures.get(&key.0) {
                    escalate |= *previous != key.1;
                } else {
                    self.failures.insert(key.0.clone(), key.1);
                    failed_rounds.push(key.1);
                }
            }
            if round.0 == 0
                && !round.1
                && self
                    .failures
                    .get(&key.0)
                    .is_some_and(|failed| key.1 > *failed)
            {
                self.failures.remove(&key.0);
            }
        }
        (failed_rounds, escalate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: i64, round: i64, status: &str) -> ToolCompletion {
        ToolCompletion {
            event_id: id,
            tool_run_id: format!("task-{id}"),
            tool_name: "read_file".into(),
            tool_call_id: format!("call-{id}"),
            dispatch_round: round,
            agent_run_id: "root".into(),
            scope_id: "child".into(),
            status: status.into(),
            error_kind: None,
            fatal: false,
            retryable: false,
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "raw".into(),
            usage_json: None,
            delivery_message_id: None,
            observed_request_step: None,
        }
    }
    #[test]
    fn failures_in_one_round_only_use_one_retry() {
        let mut policy = FailurePolicy::default();
        policy.register(["read_file".into(), "read_file".into()].into_iter(), 1);
        assert_eq!(
            policy.ingest([event(1, 1, "failed"), event(2, 1, "failed")].iter()),
            (vec![1], false)
        );
        policy.register(["read_file".into()].into_iter(), 2);
        assert!(policy.ingest([event(3, 2, "failed")].iter()).1);
    }
    #[test]
    fn delayed_old_success_cannot_erase_new_failure() {
        let mut policy = FailurePolicy::default();
        policy.register(["read_file".into()].into_iter(), 1);
        policy.register(["read_file".into()].into_iter(), 2);
        assert!(!policy.ingest([event(2, 2, "failed")].iter()).1);
        assert!(!policy.ingest([event(1, 1, "succeeded")].iter()).1);
        assert!(policy.ingest([event(3, 3, "failed")].iter()).1);
    }
    #[test]
    fn entire_new_successful_round_resets_retry() {
        let mut policy = FailurePolicy::default();
        assert!(!policy.ingest([event(1, 1, "failed")].iter()).1);
        policy.register(["read_file".into(), "read_file".into()].into_iter(), 2);
        policy.ingest([event(2, 2, "succeeded"), event(3, 2, "succeeded")].iter());
        assert_eq!(
            policy.ingest([event(4, 3, "failed")].iter()),
            (vec![3], false)
        );
    }
}
