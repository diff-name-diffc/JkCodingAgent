//! 预览是有期限的观察者投影，不占浏览器执行权，也不触发浏览器启动。
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

use super::process::BrowserProcess;
use super::BrowserManager;

const LEASE: Duration = Duration::from_secs(8);
const MAX_OBSERVERS: usize = 32;

struct Observer {
    tool_call_id: Option<String>,
    expires: Instant,
}

#[derive(Default)]
struct Leases {
    revision: u64,
    observers: HashMap<String, Observer>,
}

pub(super) struct PreviewRegistry {
    leases: Mutex<Leases>,
    changes: watch::Sender<u64>,
}

impl Default for PreviewRegistry {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            leases: Mutex::new(Leases::default()),
            changes,
        }
    }
}

impl BrowserManager {
    pub async fn preview_set(
        &self,
        session_id: &str,
        observer_id: String,
        tool_call_id: Option<String>,
        visible: bool,
    ) -> Result<(), String> {
        validate_id("会话", session_id)?;
        validate_id("观察者", &observer_id)?;
        if let Some(id) = &tool_call_id {
            validate_id("工具调用", id)?;
        }
        let session = if visible {
            self.session(session_id).await
        } else {
            let Some(session) = self.sessions.lock().get(session_id).cloned() else {
                return Ok(());
            };
            session
        };
        session
            .preview
            .set(observer_id, tool_call_id, visible, Instant::now())
    }

    pub fn cancel_exploration(&self, session_id: &str, tool_call_id: &str) -> Result<(), String> {
        validate_id("会话", session_id)?;
        validate_id("工具调用", tool_call_id)?;
        let session = self
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| "目标浏览器探索已结束或会话不存在".to_string())?;
        session.cancel_exploration(tool_call_id)
    }
}

fn validate_id(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 256 {
        Err(format!("浏览器{kind}标识必须为 1–256 字节"))
    } else {
        Ok(())
    }
}

impl PreviewRegistry {
    fn set(
        &self,
        id: String,
        tool_call_id: Option<String>,
        visible: bool,
        now: Instant,
    ) -> Result<(), String> {
        let mut leases = self.leases.lock();
        leases
            .observers
            .retain(|_, observer| observer.expires > now);
        if visible && !leases.observers.contains_key(&id) && leases.observers.len() >= MAX_OBSERVERS
        {
            return Err(format!(
                "同一浏览器会话最多支持 {MAX_OBSERVERS} 个预览观察者"
            ));
        }
        leases.revision = leases
            .revision
            .checked_add(1)
            .ok_or_else(|| "浏览器预览版本号已耗尽".to_string())?;
        if visible {
            leases.observers.insert(
                id,
                Observer {
                    tool_call_id,
                    expires: now + LEASE,
                },
            );
        } else {
            leases.observers.remove(&id);
        }
        self.changes.send_replace(leases.revision);
        Ok(())
    }

    fn snapshot(&self, now: Instant) -> Value {
        let leases = self.leases.lock();
        let observers = leases
            .observers
            .iter()
            .filter_map(|(id, observer)| {
                let lease_ms = observer
                    .expires
                    .saturating_duration_since(now)
                    .as_millis()
                    .min(LEASE.as_millis()) as u64;
                (lease_ms > 0).then(|| {
                    json!({"observerId":id,"toolCallId":observer.tool_call_id,"leaseMs":lease_ms})
                })
            })
            .collect::<Vec<_>>();
        json!({"revision":leases.revision,"observers":observers})
    }

    pub(super) fn attach(self: &Arc<Self>, process: &Arc<BrowserProcess>) {
        if !process.claim_preview_worker() {
            return;
        }
        let registry = Arc::downgrade(self);
        let weak_process = Arc::downgrade(process);
        let mut changes = self.changes.subscribe();
        let mut closed = process.termination_receiver();
        tokio::spawn(async move {
            loop {
                changes.borrow_and_update();
                let Some(registry) = registry.upgrade() else {
                    break;
                };
                let snapshot = registry.snapshot(Instant::now());
                drop(registry);
                let Some(process) = weak_process.upgrade().filter(|process| process.is_alive())
                else {
                    break;
                };
                let result = {
                    let request = process.request("set_preview", snapshot);
                    tokio::pin!(request);
                    tokio::select! {
                        biased;
                        _ = closed.changed() => break,
                        result = &mut request => result,
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {
                            // 这里只报告观察失败，不能 drop 共享 JSONL 的写入 future。
                            // 原请求结算前，watch 保留最新 revision，不发送第二个请求。
                            if process.is_alive() {
                                process.report_preview_error("浏览器预览同步超过 2 秒".into());
                            }
                            tokio::select! {
                                biased;
                                _ = closed.changed() => break,
                                result = &mut request => result,
                            }
                        }
                    }
                };
                if let Err(error) = result {
                    if process.is_alive() {
                        process.report_preview_error(error);
                    }
                }
                drop(process);
                tokio::select! {
                    biased;
                    _ = closed.changed() => break,
                    result = changes.changed() => if result.is_err() { break; },
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_timeout;
