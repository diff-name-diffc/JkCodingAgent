use super::*;
use crate::browser::process;
use tokio::process::Command;
use tokio::sync::Notify;

#[tokio::test]
async fn timeout_keeps_request_alive_until_late_ack_then_sends_only_latest_revision() {
    let previews = Arc::new(Mutex::new(Vec::new()));
    let preview_ready = Arc::new(Notify::new());
    let timeout_reported = Arc::new(Notify::new());
    let (observed, ready, failed) = (
        previews.clone(),
        preview_ready.clone(),
        timeout_reported.clone(),
    );
    let sink: process::EventSink = Arc::new(move |name, value| {
        if name == "browser-log" && value.get("preview").is_some() {
            observed.lock().push(value["preview"].clone());
            ready.notify_one();
        }
        if name == "browser-preview-error" {
            failed.notify_one();
        }
    });
    // 首个预览 ACK 由独立协议命令放行；服务持续读 stdin，能识别违规的并发第二请求。
    let mut command = Command::new("python3");
    command.args([
        "-u",
        "-c",
        r#"
import json,sys
count=0
pending=None
for line in sys.stdin:
    req=json.loads(line)
    if req['method']=='set_preview':
        count+=1
        print(json.dumps({'event':'log','sessionId':'session','preview':req['params']}),flush=True)
        if count==1:
            pending=req['id']
            continue
    if req['method']=='release_preview':
        print(json.dumps({'id':pending,'ok':True,'result':{}}),flush=True)
    print(json.dumps({'id':req['id'],'ok':True,'result':{'previewCount':count}}),flush=True)
"#,
    ]);
    let process = process::spawn(command, "session".into(), sink).unwrap();
    let registry = Arc::new(PreviewRegistry::default());
    registry
        .set("view".into(), Some("tool".into()), true, Instant::now())
        .unwrap();
    registry.attach(&process);
    tokio::time::timeout(Duration::from_secs(2), preview_ready.notified())
        .await
        .unwrap();

    tokio::time::pause();
    for _ in 0..20 {
        registry
            .set("view".into(), Some("tool".into()), true, Instant::now())
            .unwrap();
    }
    registry
        .set("view".into(), None, false, Instant::now())
        .unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    timeout_reported.notified().await;
    tokio::task::yield_now().await;
    tokio::time::resume();

    // 模拟主 Agent 继续使用相同 JSONL 管道，既不能被杀，也不能收到破损协议。
    let probe = tokio::time::timeout(Duration::from_secs(2), process.request("probe", json!({})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        probe["previewCount"], 1,
        "超时报告不能切断未确认请求并发出第二个预览"
    );
    assert!(process.is_alive());
    process.request("release_preview", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), preview_ready.notified())
        .await
        .unwrap();
    assert_eq!(previews.lock().len(), 2);
    assert_eq!(previews.lock()[1]["revision"], 22);
    assert_eq!(previews.lock()[1]["observers"], json!([]));
    process.terminate("测试回收").await.unwrap();
}
