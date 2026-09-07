//! RAG sidecar 失败原因记录（UI-22c 遗留登记）：`rag_status` 内联透出真实
//! 失败原因，替代「未运行时只能翻服务日志」。仅内存态，不落库、不进 schema。
//!
//! 独立小模块承载（manager.rs 已超仓库 500 行规模红线，新增能力不再膨胀该
//! 文件）：`RagFailureLog` 以内部 `Arc` 实现 Clone，供 wait reaper 等
//! `'static` 任务持有共享写入端；所有方法临界区只做内存读写（持锁禁 I/O）。
//!
//! 第二批增强（UI-22c 登记遗留）：`RagStderrRing` 环形缓冲保留 sidecar
//! 最近 N 行 stderr（脱敏后），失败原因拼接尾部片段——「握手前已退出」
//! 等场景的真因常只在 stderr，此前需打开服务日志面板才能看到。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::logs::redact_log_text;

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

/// stderr 环形缓冲容量（行）。
pub const STDERR_RING_CAPACITY: usize = 20;
/// 单行入环前的截断上限（chars）。
const STDERR_LINE_MAX_CHARS: usize = 300;
/// 拼接失败原因时取用的最近行数。
const STDERR_TAIL_MAX_LINES: usize = 5;
/// 拼接尾串总长钳制（chars），超限截尾加「…」。
const STDERR_TAIL_TOTAL_MAX_CHARS: usize = 600;
/// 进程退出/早退路径取快照前的 drain 宽限：stderr reader 可能滞后于
/// wait()/握手通道关闭，短窗口让致命尾部行落进环形缓冲。仅失败路径
/// 支付该延迟，成功路径零影响。
pub const STDERR_DRAIN_GRACE: Duration = Duration::from_millis(80);

/// sidecar stderr 最近 N 行的环形缓冲，失败原因拼接用。
///
/// 按代隔离：每次 spawn 新建一个 ring，旧 ring 随其 reader/reaper 任务
/// 结束自然 drop，restart 后不会拼进上一代的陈旧尾巴。Clone 共享（内部
/// Arc），供 `'static` reader/reaper 任务持有写入端；push 在锁外完成
/// 脱敏/trim/截断，临界区仅 VecDeque 内存操作（持锁禁 I/O）。
///
/// 脱敏复用 `logs::redact_log_text`（与 RagLogStore 同一函数）且在 push
/// 内部执行——ring 自身保证永不持密，凭据不会经 `rag_status` 泄出。
#[derive(Clone, Default)]
pub struct RagStderrRing {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl RagStderrRing {
    /// 压入一行 stderr（脱敏 + trim + 截断；空行跳过）。
    pub fn push(&self, line: &str) {
        let cleaned = redact_log_text(line).trim().to_string();
        if cleaned.is_empty() {
            return;
        }
        let cleaned: String = cleaned.chars().take(STDERR_LINE_MAX_CHARS).collect();
        let mut ring = self.inner.lock();
        ring.push_back(cleaned);
        while ring.len() > STDERR_RING_CAPACITY {
            ring.pop_front();
        }
    }

    /// 当前快照（clone 出临界区）。
    pub fn snapshot(&self) -> Vec<String> {
        self.inner.lock().iter().cloned().collect()
    }

    /// 失败原因拼接：base + stderr 尾部；空快照原样返回 base（no-op，
    /// 故所有失败点可无条件调用）。
    pub fn failure_message(&self, base: &str) -> String {
        append_stderr_tail(base, &self.snapshot())
    }
}

/// 拼接格式：`{base}｜stderr 尾部：{最近≤5行以 " / " 连接}`，总长钳制。
/// 单行化——前端以内联 span + title tooltip 渲染 reason，多行会撑破状态行。
fn append_stderr_tail(message: &str, tail: &[String]) -> String {
    if tail.is_empty() {
        return message.to_string();
    }
    let start = tail.len().saturating_sub(STDERR_TAIL_MAX_LINES);
    let joined = tail[start..].join(" / ");
    let clipped = if joined.chars().count() > STDERR_TAIL_TOTAL_MAX_CHARS {
        let cut: String = joined.chars().take(STDERR_TAIL_TOTAL_MAX_CHARS).collect();
        format!("{cut}…")
    } else {
        joined
    };
    format!("{message}｜stderr 尾部：{clipped}")
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

    // ── RagStderrRing（UI-22c 登记增强）──────────────────────────────────

    #[test]
    fn stderr_ring_capacity_fifo() {
        let ring = RagStderrRing::default();
        for i in 0..25 {
            ring.push(&format!("line{i}"));
        }
        let snapshot = ring.snapshot();
        assert_eq!(snapshot.len(), STDERR_RING_CAPACITY, "容量应为 20 行");
        assert_eq!(snapshot.first().map(String::as_str), Some("line5"), "最旧 5 行应被淘汰");
        assert_eq!(snapshot.last().map(String::as_str), Some("line24"), "最新行应保留");
    }

    #[test]
    fn stderr_ring_skips_blank_lines() {
        let ring = RagStderrRing::default();
        ring.push("   ");
        ring.push("");
        assert!(ring.snapshot().is_empty(), "空白行不入环");
    }

    #[test]
    fn stderr_ring_truncates_long_line() {
        let ring = RagStderrRing::default();
        ring.push(&"x".repeat(400));
        let snapshot = ring.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].chars().count(), 300, "单行应截断到 300 chars");
    }

    #[test]
    fn stderr_ring_redacts_secrets_before_storing() {
        let ring = RagStderrRing::default();
        ring.push("key sk-secret123 end");
        ring.push("Authorization: Bearer x");
        let snapshot = ring.snapshot();
        assert_eq!(snapshot[0], "key sk-*** end", "sk- 凭据应脱敏");
        assert!(!snapshot[0].contains("sk-secret123"));
        assert!(snapshot[1].contains("Authorization: ***"), "Authorization 应脱敏");
    }

    #[test]
    fn append_stderr_tail_format() {
        let tail = vec!["l1".to_string(), "l2".to_string()];
        assert_eq!(append_stderr_tail("base", &tail), "base｜stderr 尾部：l1 / l2");
    }

    #[test]
    fn append_stderr_tail_empty_is_noop() {
        assert_eq!(append_stderr_tail("base", &[]), "base", "空快照应原样返回");
    }

    #[test]
    fn append_stderr_tail_clamps_lines_and_total_length() {
        // 8 行只取最近 5 行
        let tail: Vec<String> = (0..8).map(|i| format!("l{i}")).collect();
        let joined = append_stderr_tail("base", &tail);
        assert_eq!(joined, "base｜stderr 尾部：l3 / l4 / l5 / l6 / l7");

        // 总长超 600 chars 截尾加「…」
        let long_line = "y".repeat(200);
        let tail = vec![long_line; 5];
        let joined = append_stderr_tail("base", &tail);
        let tail_part = joined.split_once("｜stderr 尾部：").expect("拼接").1;
        assert_eq!(tail_part.chars().count(), 601, "600 钳制 + 省略号");
        assert!(tail_part.ends_with('…'));
    }

    #[test]
    fn stderr_ring_generations_isolated_and_failure_message_integrates() {
        let ring_a = RagStderrRing::default();
        let ring_b = ring_a.clone();
        ring_a.push("fatal: boom");
        assert_eq!(ring_b.snapshot(), ring_a.snapshot(), "clone 共享同一环");

        let fresh = RagStderrRing::default();
        assert!(fresh.snapshot().is_empty(), "新代 ring 独立于旧代");
        assert_eq!(fresh.failure_message("base"), "base", "空环 failure_message 为 no-op");
        assert_eq!(
            ring_a.failure_message("rag-server sidecar 在端口握手前已退出"),
            "rag-server sidecar 在端口握手前已退出｜stderr 尾部：fatal: boom"
        );
    }
}
