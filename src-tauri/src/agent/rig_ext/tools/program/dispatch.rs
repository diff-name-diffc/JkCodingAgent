//! 绑定调用的提交序调度。
//!
//! 连续的可并行调用重叠到上限。未标记可并行的调用先等池子排空，再单独执行。
//! 取消或 wall-time 之后不再出队；已经出队的调用由调用方等待真实结束。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::error::FailureKind;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum StopKind {
    Timeout,
    Abort,
}

impl StopKind {
    fn failure(self) -> FailureKind {
        match self {
            Self::Timeout => FailureKind::Timeout,
            Self::Abort => FailureKind::Abort,
        }
    }
}

#[derive(Debug)]
pub(super) enum AcquireError {
    Stopped(FailureKind),
    /// 第 33 次及以后的调用。这是程序可捕获的 `ToolCallError`，不是整次失败。
    CallLimit,
}

struct Job {
    parallel: bool,
    sequence: u64,
    start: oneshot::Sender<Result<u64, FailureKind>>,
}

struct State {
    running_parallel: usize,
    exclusive: bool,
    queue: VecDeque<Job>,
    stopped: Option<FailureKind>,
    admitted: usize,
    next_sequence: u64,
}

pub(super) struct DispatchPool {
    max_concurrency: usize,
    max_calls: usize,
    state: Mutex<State>,
    /// 解释器中断钩子读取。与 `state.stopped` 同时置位。
    interrupted: AtomicBool,
}

pub(super) struct Permit {
    pool: Arc<DispatchPool>,
    parallel: bool,
    sequence: u64,
    released: bool,
}

impl Permit {
    pub(super) fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl DispatchPool {
    pub(super) fn new(max_concurrency: usize, max_calls: usize) -> Arc<Self> {
        Arc::new(Self {
            max_concurrency: max_concurrency.max(1),
            max_calls,
            state: Mutex::new(State {
                running_parallel: 0,
                exclusive: false,
                queue: VecDeque::new(),
                stopped: None,
                admitted: 0,
                next_sequence: 0,
            }),
            interrupted: AtomicBool::new(false),
        })
    }

    pub(super) fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    pub(super) fn stop_kind(&self) -> Option<FailureKind> {
        self.state.lock().stopped
    }

    /// 解释器中断钩子。绑定还在跑时返回 false，避免丢掉已经交给叶子的 future。
    pub(super) fn should_interrupt_js(&self) -> bool {
        if !self.is_interrupted() {
            return false;
        }
        let state = self.state.lock();
        state.running_parallel == 0 && !state.exclusive
    }

    pub(super) fn call_limit_message(&self) -> String {
        format!("单次程序最多调用 {} 次工具", self.max_calls)
    }

    pub(super) fn stop(&self, kind: StopKind) {
        let mut state = self.state.lock();
        if state.stopped.is_none() {
            state.stopped = Some(kind.failure());
            self.interrupted.store(true, Ordering::Release);
        }
        pump(&mut state, self.max_concurrency);
    }

    pub(super) async fn acquire(self: &Arc<Self>, parallel: bool) -> Result<Permit, AcquireError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock();
            if let Some(kind) = state.stopped {
                return Err(AcquireError::Stopped(kind));
            }
            if state.admitted >= self.max_calls {
                return Err(AcquireError::CallLimit);
            }
            state.admitted += 1;
            state.next_sequence += 1;
            let sequence = state.next_sequence;
            state.queue.push_back(Job {
                parallel,
                sequence,
                start: tx,
            });
            pump(&mut state, self.max_concurrency);
        }
        match rx.await {
            Ok(Ok(sequence)) => Ok(Permit {
                pool: Arc::clone(self),
                parallel,
                sequence,
                released: false,
            }),
            Ok(Err(kind)) => Err(AcquireError::Stopped(kind)),
            Err(_) => Err(AcquireError::Stopped(FailureKind::Abort)),
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut state = self.pool.state.lock();
        if self.parallel {
            state.running_parallel = state.running_parallel.saturating_sub(1);
        } else {
            state.exclusive = false;
        }
        pump(&mut state, self.pool.max_concurrency);
    }
}

fn pump(state: &mut State, max_concurrency: usize) {
    if let Some(kind) = state.stopped {
        while let Some(job) = state.queue.pop_front() {
            let _ = job.start.send(Err(kind));
        }
        return;
    }
    while let Some(front) = state.queue.front() {
        if state.exclusive {
            break;
        }
        if front.parallel {
            if state.running_parallel >= max_concurrency {
                break;
            }
            let job = state.queue.pop_front().expect("front exists");
            state.running_parallel += 1;
            let _ = job.start.send(Ok(job.sequence));
        } else if state.running_parallel > 0 {
            break;
        } else {
            let job = state.queue.pop_front().expect("front exists");
            state.exclusive = true;
            let _ = job.start.send(Ok(job.sequence));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn parallel_calls_overlap_and_exclusive_calls_do_not() {
        let pool = DispatchPool::new(4, 32);
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_parallel = Arc::new(AtomicUsize::new(0));
        let max_exclusive = Arc::new(AtomicUsize::new(0));

        async fn body(inflight: Arc<AtomicUsize>, max_seen: Arc<AtomicUsize>) {
            let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            inflight.fetch_sub(1, Ordering::SeqCst);
        }

        let first_inflight = Arc::clone(&inflight);
        let first_max = Arc::clone(&max_parallel);
        let second_inflight = Arc::clone(&inflight);
        let second_max = Arc::clone(&max_parallel);
        let left = pool.acquire(true);
        let right = pool.acquire(true);
        let (left, right) = tokio::join!(left, right);
        let left = left.expect("first permit");
        let right = right.expect("second permit");
        let ((), ()) = tokio::join!(
            body(first_inflight, first_max),
            body(second_inflight, second_max)
        );
        drop(left);
        drop(right);
        assert!(max_parallel.load(Ordering::SeqCst) >= 2);

        inflight.store(0, Ordering::SeqCst);
        let exclusive_pool = DispatchPool::new(4, 32);
        let left = exclusive_pool.acquire(false);
        let right = exclusive_pool.acquire(false);
        let left = left.await.expect("exclusive starts immediately");
        let inflight_a = Arc::clone(&inflight);
        let max_a = Arc::clone(&max_exclusive);
        let ran = tokio::spawn(async move {
            body(inflight_a, max_a).await;
            drop(left);
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let right = right.await.expect("exclusive waits then starts");
        let inflight_b = Arc::clone(&inflight);
        let max_b = Arc::clone(&max_exclusive);
        body(inflight_b, max_b).await;
        drop(right);
        ran.await.expect("first exclusive finished");
        assert_eq!(max_exclusive.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stop_rejects_calls_that_have_not_started() {
        let pool = DispatchPool::new(4, 32);
        let running = pool.acquire(true).await.expect("in flight");
        pool.stop(StopKind::Abort);
        let blocked = pool.acquire(true).await;
        assert!(matches!(
            blocked,
            Err(AcquireError::Stopped(FailureKind::Abort))
        ));
        drop(running);
    }

    #[tokio::test]
    async fn call_limit_does_not_stop_the_pool() {
        let pool = DispatchPool::new(4, 1);
        let first = pool.acquire(true).await.expect("the only slot");
        let limited = pool.acquire(true).await;
        assert!(matches!(limited, Err(AcquireError::CallLimit)));
        drop(first);
    }
}
