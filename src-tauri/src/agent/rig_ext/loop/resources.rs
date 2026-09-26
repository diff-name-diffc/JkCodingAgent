//! 全资源集合原子仲裁：冲突任务遵循登记顺序，独立任务可越过等待者。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{watch, Notify};
use tokio::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resource {
    /// 路径须由宿主沙箱校验并 canonicalize，不能直接接受模型的权限声明。
    File(PathBuf),
    /// 工作区级文件域（工作区根，canonical）。带作用域是为了让无关工作区的会话
    /// 互不排队：它只与工作区内的 `File` 声明、以及同一或嵌套工作区的同类声明互斥。
    LocalFilesystem(PathBuf),
    Session(String),
    Ssh(String),
    External,
}

#[derive(Clone, Debug)]
pub(crate) struct Claim {
    pub resource: Resource,
    pub write: bool,
}

/// 路径域重叠：同一路径，或互为祖先/后代，都算重叠（保守，不看读写方向）。
fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl Claim {
    fn conflicts(&self, other: &Self) -> bool {
        if !self.write && !other.write {
            return false;
        }
        match (&self.resource, &other.resource) {
            (Resource::File(a), Resource::File(b)) => paths_overlap(a, b),
            (Resource::LocalFilesystem(scope), Resource::File(path))
            | (Resource::File(path), Resource::LocalFilesystem(scope)) => {
                paths_overlap(scope, path)
            }
            (Resource::LocalFilesystem(a), Resource::LocalFilesystem(b)) => paths_overlap(a, b),
            (a, b) => a == b,
        }
    }
}

#[derive(Default)]
struct State {
    next_id: u64,
    waiting: BTreeMap<u64, Vec<Claim>>,
    active: BTreeMap<u64, Vec<Claim>>,
}

#[derive(Default)]
pub(crate) struct ResourceArbiter {
    state: Mutex<State>,
    changed: Notify,
}

/// 排队登记和执行租约使用同一个守卫；future 被取消也不会遗留队列条目。
pub(crate) struct ResourceLease {
    arbiter: Arc<ResourceArbiter>,
    id: u64,
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        {
            let mut state = self.arbiter.state.lock();
            state.waiting.remove(&self.id);
            state.active.remove(&self.id);
        }
        self.arbiter.changed.notify_waiters();
    }
}

/// 排队超时携带超时时刻的阻塞声明：让调用方能把「谁占着资源」写进错误文案
/// （图运行全程持工作区写租约，只报「排队超时」时用户与模型都无从判断）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AcquireError {
    Cancelled,
    QueueTimeout { blocked_by: Option<Resource> },
}

impl AcquireError {
    /// 排队超时的模型可见文案。阻塞方已知时点名其种类/路径，未知时给固定文案。
    pub(crate) fn timeout_message(&self) -> String {
        match self {
            AcquireError::QueueTimeout {
                blocked_by: Some(resource),
            } => format!(
                "错误：工具资源排队超时：{} 被其它运行独占（正在运行的图编排会持有工作区写租约），请稍后重试或先结束该运行",
                resource.describe()
            ),
            _ => "错误：工具资源排队超时：所需资源被其它运行独占（正在运行的图编排会持有工作区写租约），请稍后重试或先结束该运行".into(),
        }
    }
}

impl Resource {
    /// 超时文案中的简短描述：只含资源域身份，不带工具参数。
    fn describe(&self) -> String {
        match self {
            Resource::File(path) => format!("路径 {}", path.display()),
            Resource::LocalFilesystem(scope) => format!("工作区 {}", scope.display()),
            Resource::Session(id) => format!("会话 {id}"),
            Resource::Ssh(id) => format!("SSH 服务器 {id}"),
            Resource::External => "外部资源".to_string(),
        }
    }
}

impl ResourceArbiter {
    pub(crate) fn shared() -> Arc<Self> {
        static ARBITER: std::sync::OnceLock<Arc<ResourceArbiter>> = std::sync::OnceLock::new();
        ARBITER.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub(crate) fn reserve(self: &Arc<Self>, claims: Vec<Claim>) -> ResourceReservation {
        let lease = {
            let mut state = self.state.lock();
            let id = state.next_id;
            state.next_id = state
                .next_id
                .checked_add(1)
                .expect("resource ticket overflow");
            state.waiting.insert(id, claims.clone());
            ResourceLease {
                arbiter: self.clone(),
                id,
            }
        };
        ResourceReservation { lease, claims }
    }

    pub(crate) async fn acquire(
        self: &Arc<Self>,
        claims: Vec<Claim>,
        cancel: watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<ResourceLease, AcquireError> {
        self.reserve(claims).wait(cancel, deadline).await
    }
}

/// 当前阻塞本声明的资源：真实持锁者优先，其次是登记更早的等待者（遵循登记顺序）。
fn find_blocker(state: &State, claims: &[Claim], id: u64) -> Option<Resource> {
    state
        .active
        .values()
        .chain(state.waiting.range(..id).map(|(_, other)| other))
        .find_map(|others| {
            claims.iter().find_map(|claim| {
                others
                    .iter()
                    .find(|other| claim.conflicts(other))
                    .map(|other| other.resource.clone())
            })
        })
}

pub(crate) struct ResourceReservation {
    lease: ResourceLease,
    claims: Vec<Claim>,
}
impl ResourceReservation {
    pub(crate) async fn wait(
        self,
        mut cancel: watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<ResourceLease, AcquireError> {
        let Self { lease, claims } = self;
        let arbiter = lease.arbiter.clone();
        let mut blocked_by: Option<Resource> = None;
        loop {
            // 在检查条件前注册通知，覆盖“检查与挂起之间释放”的窗口。
            let changed = arbiter.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if *cancel.borrow() || cancel.has_changed().is_err() {
                return Err(AcquireError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(AcquireError::QueueTimeout {
                    // 超时时刻再扫一次：报当前仍在的阻塞者，其次回退到上一次观测值。
                    blocked_by: find_blocker(&arbiter.state.lock(), &claims, lease.id)
                        .or(blocked_by),
                });
            }
            let acquired = {
                let mut state = arbiter.state.lock();
                match find_blocker(&state, &claims, lease.id) {
                    Some(blocker) => {
                        blocked_by = Some(blocker);
                        false
                    }
                    None => {
                        state.waiting.remove(&lease.id);
                        state.active.insert(lease.id, claims.clone());
                        true
                    }
                }
            };
            if acquired {
                return Ok(lease);
            }
            let timed_out = tokio::select! {
                _ = &mut changed => false,
                _ = cancel.changed() => false,
                _ = tokio::time::sleep_until(deadline) => true,
            };
            if timed_out {
                return Err(AcquireError::QueueTimeout {
                    blocked_by: find_blocker(&arbiter.state.lock(), &claims, lease.id)
                        .or(blocked_by),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(path: &str, write: bool) -> Vec<Claim> {
        vec![Claim {
            resource: Resource::File(path.into()),
            write,
        }]
    }

    fn claim(resource: Resource, write: bool) -> Claim {
        Claim { resource, write }
    }

    fn workspace(path: &str) -> Resource {
        Resource::LocalFilesystem(path.into())
    }

    #[test]
    fn local_filesystem_scope_conflicts_only_inside_same_or_nested_workspace() {
        // 工作区内的文件声明与工作区域级声明互斥（含父子双向）。
        assert!(claim(workspace("/repo"), true)
            .conflicts(&claim(Resource::File("/repo/a".into()), true)));
        assert!(claim(Resource::File("/repo/a".into()), true)
            .conflicts(&claim(workspace("/repo"), true)));
        assert!(
            claim(workspace("/repo"), true).conflicts(&claim(Resource::File("/repo".into()), true))
        );
        assert!(claim(workspace("/repo"), true).conflicts(&claim(Resource::File("/".into()), true)));
        // 同一或嵌套工作区互斥。
        assert!(claim(workspace("/repo"), true).conflicts(&claim(workspace("/repo"), true)));
        assert!(claim(workspace("/repo"), true).conflicts(&claim(workspace("/repo/sub"), true)));
        assert!(claim(workspace("/repo/sub"), true).conflicts(&claim(workspace("/repo"), true)));
        // 无关工作区不再互相排队（R34）。
        assert!(!claim(workspace("/repo"), true).conflicts(&claim(workspace("/other"), true)));
        assert!(!claim(workspace("/repo"), true)
            .conflicts(&claim(Resource::File("/other/a".into()), true)));
        assert!(!claim(workspace("/repo/sub"), true)
            .conflicts(&claim(Resource::File("/repo2/a".into()), true)));
        // 只读声明之间不互斥，读写与只读相对写仍互斥。
        assert!(!claim(workspace("/repo"), false)
            .conflicts(&claim(Resource::File("/repo/a".into()), false)));
        assert!(claim(workspace("/repo"), false)
            .conflicts(&claim(Resource::File("/repo/a".into()), true)));
    }

    #[tokio::test]
    async fn conflicting_writer_is_ordered_and_independent_path_passes() {
        let arbiter = Arc::new(ResourceArbiter::default());
        let (_tx, rx) = watch::channel(false);
        let deadline = Instant::now() + Duration::from_secs(60);
        let first = arbiter
            .acquire(file("/repo/a", false), rx.clone(), deadline)
            .await
            .unwrap();
        let writer = arbiter.acquire(file("/repo", true), rx.clone(), deadline);
        tokio::pin!(writer);
        assert!(futures::poll!(&mut writer).is_pending());
        let late_reader = arbiter.acquire(file("/repo/a", false), rx.clone(), deadline);
        tokio::pin!(late_reader);
        assert!(futures::poll!(&mut late_reader).is_pending());
        let independent = arbiter
            .acquire(file("/other", true), rx, deadline)
            .await
            .unwrap();
        drop(first);
        let writer = writer.await.unwrap();
        assert!(futures::poll!(&mut late_reader).is_pending());
        drop(writer);
        drop(late_reader.await.unwrap());
        drop(independent);
        assert!(arbiter.state.lock().active.is_empty());
    }

    #[tokio::test]
    async fn cancelling_waiter_releases_whole_claim_set() {
        let arbiter = Arc::new(ResourceArbiter::default());
        let (tx, rx) = watch::channel(false);
        let deadline = Instant::now() + Duration::from_secs(60);
        let lease = arbiter
            .acquire(file("/repo", true), rx.clone(), deadline)
            .await
            .unwrap();
        let waiter = arbiter.acquire(file("/repo/a", true), rx, deadline);
        tokio::pin!(waiter);
        assert!(futures::poll!(&mut waiter).is_pending());
        tx.send(true).unwrap();
        assert!(matches!(waiter.await, Err(AcquireError::Cancelled)));
        assert!(arbiter.state.lock().waiting.is_empty());
        drop(lease);
        assert!(arbiter.state.lock().active.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn queue_timeout_reports_the_blocking_resource() {
        let arbiter = Arc::new(ResourceArbiter::default());
        let (_tx, rx) = watch::channel(false);
        let holder = arbiter
            .acquire(
                file("/repo", true),
                rx.clone(),
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .unwrap();
        let timeout = arbiter
            .acquire(
                file("/repo/a", true),
                rx,
                Instant::now() + Duration::from_secs(1),
            )
            .await;
        let blocked_by = match timeout {
            Ok(_) => panic!("写 /repo 的租约已被占，应当排队超时"),
            Err(AcquireError::QueueTimeout { blocked_by }) => blocked_by,
            Err(AcquireError::Cancelled) => panic!("未取消，不应报取消"),
        };
        assert_eq!(blocked_by, Some(Resource::File("/repo".into())));
        assert!(AcquireError::QueueTimeout {
            blocked_by: Some(Resource::File("/repo".into()))
        }
        .timeout_message()
        .contains("/repo"));
        drop(holder);
    }
}
