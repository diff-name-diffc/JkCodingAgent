//! RAG sidecar 失败原因记录（UI-22c 遗留登记）：`rag_status` 内联透出真实
//! 失败原因，替代「未运行时只能翻服务日志」。仅内存态，不落库、不进 schema。
//!
//! 独立小模块承载（manager.rs 已超仓库 500 行规模红线，新增能力不再膨胀该
//! 文件）：`RagFailureLog` 以内部 `Arc` 实现 Clone，供 wait reaper 等
//! `'static` 任务持有共享写入端；所有方法临界区只做内存读写（持锁禁 I/O）。

use std::sync::Arc;

use parking_lot::Mutex;

/// 最近一次失败记录。
#[derive(Clone, Debug)]
pub struct RagFailure {
    pub message: String,
    /// 失败发生时刻（Unix epoch 毫秒）。
    pub at_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 失败记录的共享写入/读取端。语义为「最近一次失败」：
/// - 启动失败（ensure_started / restart 的 spawn+握手错误）与运行期退出
///   （wait reaper）都会 record 覆盖；
/// - 启动成功即 clear；
/// - restart 内主动 stop 触发的 reaper 记录会被随后的启动成功清空或启动
///   失败覆盖，前端仅在非运行态透出，陈旧记录不会误导。
#[derive(Clone, Default)]
pub struct RagFailureLog {
    inner: Arc<Mutex<Option<RagFailure>>>,
}

impl RagFailureLog {
    /// 记录最近失败原因（覆盖旧记录）。
    pub fn record(&self, message: impl Into<String>) {
        *self.inner.lock() = Some(RagFailure {
            message: message.into(),
            at_ms: now_ms(),
        });
    }

    /// 启动成功即清空失败记录。
    pub fn clear(&self) {
        *self.inner.lock() = None;
    }

    /// 读取最近失败记录（clone 出临界区，供命令层序列化）。
    pub fn get(&self) -> Option<RagFailure> {
        self.inner.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_get_roundtrip() {
        let log = RagFailureLog::default();
        assert!(log.get().is_none());
        log.record("启动 rag-server sidecar 失败");
        let failure = log.get().expect("recorded");
        assert_eq!(failure.message, "启动 rag-server sidecar 失败");
        assert!(failure.at_ms > 0);
    }

    #[test]
    fn clear_removes_record() {
        let log = RagFailureLog::default();
        log.record("握手超时");
        log.clear();
        assert!(log.get().is_none());
    }

    #[test]
    fn record_overwrites_previous() {
        let log = RagFailureLog::default();
        log.record("旧原因");
        log.record("新原因");
        assert_eq!(log.get().expect("recorded").message, "新原因");
    }

    #[test]
    fn clone_shares_same_record() {
        let log = RagFailureLog::default();
        let shared = log.clone();
        shared.record("来自 reaper 的退出记录");
        assert_eq!(
            log.get().expect("shared").message,
            "来自 reaper 的退出记录"
        );
        log.clear();
        assert!(shared.get().is_none());
    }
}
