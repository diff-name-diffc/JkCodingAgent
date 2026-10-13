use super::*;
use crate::browser::process;
use tokio::process::Command;
use tokio::sync::Notify;

#[tokio::test]
async fn leases_are_bounded_independent_and_expire_without_starting_browser() {
    let manager = BrowserManager::default();
    manager
        .preview_set("session", "view".into(), Some("tool".into()), true)
        .await
        .unwrap();
    let session = manager.session("session").await;
    assert!(session.process().is_none());
    let now = Instant::now();
    let registry = PreviewRegistry::default();
    for i in 0..MAX_OBSERVERS {
        registry
            .set(i.to_string(), Some("tool".into()), true, now)
            .unwrap();
    }
    assert!(registry.set("overflow".into(), None, true, now).is_err());
    registry.set("0".into(), None, false, now).unwrap();
    registry
        .set("manual".into(), None, true, now + Duration::from_secs(3))
        .unwrap();
    let snapshot = registry.snapshot(now + Duration::from_secs(8));
    assert_eq!(snapshot["observers"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["observers"][0]["observerId"], "manual");
    assert_eq!(snapshot["observers"][0]["leaseMs"], 3_000);
    assert_eq!(snapshot["observers"][0]["toolCallId"], Value::Null);
    registry
        .set(
            "replacement".into(),
            None,
            true,
            now + Duration::from_secs(12),
        )
        .unwrap();
    assert_eq!(
        registry.snapshot(now + Duration::from_secs(12))["observers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn attach_projects_existing_leases_and_coalesces_changes_while_ipc_is_pending() {
    let fixture = crate::test_util::TempDirGuard::new("browser-preview-worker");
    let release = fixture.path().join("release");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let changed = Arc::new(Notify::new());
    let observed = seen.clone();
    let notify = changed.clone();
    let sink: process::EventSink = Arc::new(move |name, value| {
        if name == "browser-log" && value.get("preview").is_some() {
            observed.lock().push(value["preview"].clone());
            notify.notify_one();
        }
    });
    let mut command = Command::new("python3");
    command
        .args([
            "-u",
            "-c",
            r#"
import json,pathlib,sys,time
release=pathlib.Path(sys.argv[1])
for line in sys.stdin:
    req=json.loads(line)
    if req['method']=='set_preview':
        print(json.dumps({'event':'log','sessionId':'session','preview':req['params']}),flush=True)
        while not release.exists(): time.sleep(.005)
    print(json.dumps({'id':req['id'],'ok':True,'result':{}}),flush=True)
"#,
        ])
        .arg(&release);
    let process = process::spawn(command, "session".into(), sink).unwrap();
    let registry = Arc::new(PreviewRegistry::default());
    registry
        .set("view".into(), Some("tool".into()), true, Instant::now())
        .unwrap();
    registry.attach(&process);
    registry.attach(&process); // 同一进程只允许一个投影 worker。
    tokio::time::timeout(Duration::from_secs(2), changed.notified())
        .await
        .unwrap();
    assert_eq!(seen.lock()[0]["revision"], 1);
    assert_eq!(seen.lock()[0]["observers"][0]["observerId"], "view");
    for _ in 0..20 {
        registry
            .set("view".into(), Some("tool".into()), true, Instant::now())
            .unwrap();
    }
    registry
        .set("view".into(), None, false, Instant::now())
        .unwrap();
    std::fs::write(&release, "go").unwrap();
    tokio::time::timeout(Duration::from_secs(2), changed.notified())
        .await
        .unwrap();
    assert_eq!(seen.lock().len(), 2);
    assert_eq!(seen.lock()[1]["revision"], 22);
    assert_eq!(seen.lock()[1]["observers"], json!([]));
    assert_eq!(
        Arc::strong_count(&registry),
        1,
        "worker 不得持有 registry 强引用等待通知"
    );
    process.terminate("测试回收").await.unwrap();
}

#[tokio::test]
async fn obsolete_cancel_identity_cannot_cancel_the_next_exploration() {
    let manager = BrowserManager::default();
    let session = manager.session("session").await;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut old = session
        .enter_exploration("old".into(), deadline, None)
        .await
        .unwrap();
    old.complete();
    drop(old);
    let mut current = session
        .enter_exploration("current".into(), deadline, None)
        .await
        .unwrap();
    assert!(manager.cancel_exploration("session", "old").is_err());
    assert!(current
        .run(deadline, None, Some("run"), async { Ok(()) })
        .await
        .is_ok());
    manager.cancel_exploration("session", "current").unwrap();
    let error = current
        .run::<()>(deadline, None, Some("run"), std::future::pending())
        .await
        .unwrap_err();
    assert!(error.starts_with("[cancelled]"));
    current.complete();
}
