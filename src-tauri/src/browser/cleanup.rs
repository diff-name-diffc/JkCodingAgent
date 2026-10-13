//! 删除/清空会话时先停止外部进程并回收浏览器数据，租约持续至数据库操作结束。
use super::{
    profile,
    session::{Operation, Session},
    BrowserManager,
};
use crate::agent::db::DispatcherDb;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{Duration, Instant};

pub struct BrowserCleanupGuard {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    session_id: String,
    session: Arc<Session>,
    _operation: Operation,
}

impl Drop for BrowserCleanupGuard {
    fn drop(&mut self) {
        let mut sessions = self.sessions.lock();
        if sessions
            .get(&self.session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.session))
        {
            sessions.remove(&self.session_id);
        }
    }
}

impl BrowserManager {
    pub async fn remove_session(
        &self,
        db: &DispatcherDb,
        session_id: &str,
    ) -> Result<BrowserCleanupGuard, String> {
        let session = self.session(session_id).await;
        session.stop().await?;
        let guard = self.lock_for_cleanup(session_id, session).await?;
        let db = db.clone();
        let session_id = session_id.to_string();
        // 即使调用 future 被丢弃，文件删除任务仍持执行租约，不能与新浏览器并发。
        tokio::task::spawn_blocking(move || {
            let workspace = profile::workspace_for_session(&db, &session_id)?;
            profile::remove_session_profile(&workspace, &session_id)?;
            Ok(guard)
        })
        .await
        .map_err(|error| format!("清理浏览器会话任务失败：{error}"))?
    }

    pub(super) async fn lock_for_cleanup(
        &self,
        session_id: &str,
        session: Arc<Session>,
    ) -> Result<BrowserCleanupGuard, String> {
        let mut operation = session
            .enter(false, Instant::now() + Duration::from_secs(10), None)
            .await?;
        session.mark_removing();
        operation.complete();
        let guard = BrowserCleanupGuard {
            sessions: self.sessions.clone(),
            session_id: session_id.into(),
            session: session.clone(),
            _operation: operation,
        };
        // 第一次 stop 只为打断占用者；等待锁期间其他手动请求可能重新启动服务。
        // 在持锁并禁止新请求后再确认一次终止，之后才允许删除 profile。
        session.stop().await?;
        Ok(guard)
    }
}
