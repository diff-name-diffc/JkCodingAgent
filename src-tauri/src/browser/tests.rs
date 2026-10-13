//! 假服务覆盖真实管道、生命周期与并发执行权；不启动浏览器或访问网络。
use super::{process, session::Session};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

pub(super) fn fake_process(
    events: process::EventSink,
) -> (crate::test_util::TempDirGuard, Arc<process::BrowserProcess>) {
    let dir = crate::test_util::TempDirGuard::new("browser-python-protocol");
    let script = dir.path().join("driver.py");
    std::fs::write(&script, r#"
import json, sys, time
for line in sys.stdin:
    req=json.loads(line)
    method=req['method']
    if method=='exit':
        print('intentional fake service failure',file=sys.stderr,flush=True)
        sys.exit(9)
    if method=='hang':
        time.sleep(30)
    if method=='emit':
        for run in ['stale', 'current']:
            print(json.dumps({'event':'agent','sessionId':'session','runId':run,'kind':'progress','data':{}}),flush=True)
        print(json.dumps({'event':'agent','sessionId':'other-session','runId':'current','kind':'progress','data':{}}),flush=True)
    if method=='child':
        import subprocess
        subprocess.Popen([sys.executable,'-c',"import time,pathlib; time.sleep(.4); pathlib.Path("+repr(req['params']['marker'])+").write_text('late')"])
        time.sleep(30)
    print(json.dumps({'id':req['id'],'ok':True,'result':{'ok':True}}),flush=True)
"#).expect("写入假服务");
    let mut command = Command::new("python3");
    command.arg("-u").arg(script);
    let process = process::spawn(command, "session".into(), events).expect("启动假服务");
    (dir, process)
}
fn sink() -> process::EventSink {
    Arc::new(|_, _| {})
}

#[tokio::test]
async fn cancellation_terminates_process_instead_of_only_dropping_response() {
    let (_dir, process) = fake_process(sink());
    let session = Arc::new(Session::default());
    *session.process.lock() = Some(process.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    let (_tx, rx) = watch::channel(false);
    let operation = session
        .enter(true, deadline, Some(rx.clone()))
        .await
        .unwrap();
    let stop = session.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        stop.stop().await.unwrap();
    });
    let result = operation
        .run(
            deadline,
            Some(rx),
            Some("run"),
            process.request("hang", json!({})),
        )
        .await;
    assert!(result.unwrap_err().starts_with("[cancelled]"));
    assert!(!process.is_alive());
    assert_eq!(process.status().state, "closed");
}

#[tokio::test]
async fn deadline_terminates_process_and_descendants() {
    let (dir, process) = fake_process(sink());
    let marker = dir.path().join("late-effect");
    let session = Arc::new(Session::default());
    *session.process.lock() = Some(process.clone());
    let deadline = Instant::now() + Duration::from_millis(150);
    let operation = session.enter(false, deadline, None).await.unwrap();
    let result = operation
        .run(
            deadline,
            None,
            None,
            process.request("child", json!({"marker":marker})),
        )
        .await;
    assert!(result.unwrap_err().starts_with("[timeout]"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!marker.exists(), "超时后子进程不得继续产生副作用");
    assert!(!process.is_alive());
}

#[tokio::test]
async fn dropped_operation_kills_in_flight_process() {
    let (_dir, process) = fake_process(sink());
    let session = Arc::new(Session::default());
    *session.process.lock() = Some(process.clone());
    let operation = session
        .enter(true, Instant::now() + Duration::from_secs(5), None)
        .await
        .unwrap();
    drop(operation);
    assert!(!process.is_alive());
    process.terminate("测试回收").await.unwrap();
}

#[tokio::test]
async fn exploration_rejects_manual_operations_and_releases_after_completion() {
    let session = Arc::new(Session::default());
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut operation = session.enter(true, deadline, None).await.unwrap();
    assert!(session
        .enter(false, deadline, None)
        .await
        .err()
        .unwrap()
        .contains("探索 Agent"));
    operation.complete();
    drop(operation);
    let mut manual = session.enter(false, deadline, None).await.unwrap();
    manual.complete();
}

#[tokio::test]
async fn simultaneous_startups_share_one_process_under_session_gate() {
    let session = Arc::new(Session::default());
    let starts = Arc::new(AtomicUsize::new(0));
    let launch = |session: Arc<Session>, starts: Arc<AtomicUsize>| async move {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut operation = session.enter(false, deadline, None).await.unwrap();
        let fixture = if session.process().is_none() {
            starts.fetch_add(1, Ordering::SeqCst);
            let (dir, process) = fake_process(sink());
            *session.process.lock() = Some(process.clone());
            process.request("start", json!({})).await.unwrap();
            Some(dir)
        } else {
            None
        };
        operation.complete();
        fixture
    };
    let (_a, _b) = tokio::join!(
        launch(session.clone(), starts.clone()),
        launch(session.clone(), starts.clone())
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    session.stop().await.unwrap();
}

#[tokio::test]
async fn only_matching_run_and_session_events_reach_callback() {
    let (_dir, process) = fake_process(sink());
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let callback_seen = seen.clone();
    process.set_agent(
        "current".into(),
        "tool-current".into(),
        Arc::new(move |event| callback_seen.lock().push(event)),
    );
    process.request("emit", json!({})).await.unwrap();
    assert_eq!(seen.lock().len(), 1);
    assert_eq!(seen.lock()[0]["runId"], "current");
    process.finish_agent();
    process.request("emit", json!({})).await.unwrap();
    assert_eq!(seen.lock().len(), 1, "已结束run的迟到事件必须被丢弃");
    process.terminate("测试回收").await.unwrap();
}

#[tokio::test]
async fn crashed_service_is_closed_and_cannot_be_reused_as_ready() {
    let (_dir, process) = fake_process(sink());
    assert!(process.request("exit", json!({})).await.is_err());
    process.terminate("测试回收").await.unwrap();
    assert!(!process.is_alive());
    assert_eq!(process.status().state, "closed");
    assert!(process
        .request("start", json!({}))
        .await
        .unwrap_err()
        .contains("已经退出"));
    let (_next_dir, replacement) = fake_process(sink());
    assert!(replacement.request("start", json!({})).await.is_ok());
    replacement.terminate("测试回收").await.unwrap();
}

#[test]
fn old_process_events_cannot_override_replacement_status() {
    let session = Arc::new(Session::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let sink: process::EventSink = Arc::new(move |_, value| observed.lock().push(value));
    let old = session.event_sink(sink.clone());
    old("browser-status", json!({"state":"ready"}));
    let current = session.event_sink(sink);
    current("browser-status", json!({"state":"starting"}));
    old("browser-status", json!({"state":"closed"}));
    assert_eq!(events.lock().len(), 2);
    assert_eq!(events.lock()[1]["state"], "starting");
}

#[tokio::test]
async fn cleanup_closes_process_removes_artifacts_and_holds_slot_until_database_work_finishes() {
    use crate::agent::db::DispatcherDb;
    use crate::project::storage::Project;
    let temp = crate::test_util::TempDirGuard::new("browser-cleanup");
    let db = DispatcherDb::new(temp.path().join("db.sqlite3")).unwrap();
    let workspace = temp.path().join("project");
    db.save_projects_all(&[Project {
        id: "p".into(),
        name: "project".into(),
        path: workspace.to_string_lossy().into_owned(),
        last_opened_at: 0,
    }])
    .unwrap();
    let record = db.create_project_session("p", "session").unwrap();
    let other = db.create_project_session("p", "other").unwrap();
    let chrome = temp.path().join("chrome");
    std::fs::create_dir_all(chrome.join("Default")).unwrap();
    std::fs::write(chrome.join("Local State"), "{}").unwrap();
    std::fs::write(chrome.join("Default/Preferences"), "{}").unwrap();
    let imported = super::profile::import_chrome_profile_blocking(
        workspace.clone(),
        chrome.clone(),
        &record.id,
    )
    .unwrap();
    let untouched =
        super::profile::import_chrome_profile_blocking(workspace, chrome, &other.id).unwrap();
    let root = std::path::PathBuf::from(imported.target_path)
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::create_dir_all(root.join("profile/runs/task")).unwrap();
    std::fs::write(root.join("profile/runs/task/report.txt"), "artifact").unwrap();
    let (_fixture, process) = fake_process(sink());
    let manager = super::BrowserManager::default();
    let session = manager.session(&record.id).await;
    *session.process.lock() = Some(process.clone());
    let cleanup = manager.remove_session(&db, &record.id).await.unwrap();
    assert!(!process.is_alive());
    assert!(!root.exists());
    assert!(std::path::Path::new(&untouched.target_path).exists());
    assert!(session
        .enter(false, Instant::now() + Duration::from_secs(1), None)
        .await
        .err()
        .unwrap()
        .contains("清理"));
    assert!(manager.sessions.lock().contains_key(&record.id));
    // 清空消息保留会话；清理租约结束后用户能够重新打开全新浏览器。
    db.clear_messages(&record.id).unwrap();
    drop(cleanup);
    assert!(!manager.sessions.lock().contains_key(&record.id));
    assert!(!Arc::ptr_eq(&manager.session(&record.id).await, &session));
}

#[tokio::test]
async fn cleanup_rechecks_process_after_acquiring_ownership() {
    let manager = super::BrowserManager::default();
    let session = manager.session("session").await;
    let (_old_dir, old) = fake_process(sink());
    *session.process.lock() = Some(old.clone());
    // 模拟删除入口先打断占用者，随后另一个手动请求在清理获得锁之前启动服务。
    session.stop().await.unwrap();
    let mut manual = session
        .enter(false, Instant::now() + Duration::from_secs(2), None)
        .await
        .unwrap();
    let (_new_dir, replacement) = fake_process(sink());
    *session.process.lock() = Some(replacement.clone());
    manual.complete();
    drop(manual);
    let cleanup = manager
        .lock_for_cleanup("session", session.clone())
        .await
        .unwrap();
    assert!(
        !replacement.is_alive(),
        "清理 profile 前必须终止拿锁之前重启的服务"
    );
    assert!(replacement.has_exited());
    drop(cleanup);
}

#[tokio::test]
async fn stop_cancels_previously_queued_operations() {
    let session = Arc::new(Session::default());
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut owner = session.enter(false, deadline, None).await.unwrap();
    let waiting = session.clone();
    let (started, queued) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(async move {
        started.send(()).unwrap();
        waiting.enter(true, deadline, None).await
    });
    queued.await.unwrap();
    session.stop().await.unwrap();
    owner.complete();
    drop(owner);
    assert!(request
        .await
        .unwrap()
        .err()
        .unwrap()
        .starts_with("[cancelled]"));
}
