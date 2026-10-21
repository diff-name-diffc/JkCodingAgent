use super::*;
use rig::message::ToolFunction;
use rig::tool::ToolOutput;

/// 临时工作区：`root` 为工作区，`base` 为其父目录（越界路径的落点）。
/// 目录生命周期由守卫统一管理（Drop 整目录回收），不再手写 Drop。
struct Workspace {
    _guard: crate::test_util::TempDirGuard,
    base: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let guard = crate::test_util::TempDirGuard::new("rig-claims");
        let base = guard.path().canonicalize().expect("canonicalize temp root");
        let root = base.join("ws");
        std::fs::create_dir_all(&root).expect("create workspace");
        std::fs::write(root.join("a.txt"), "inner").expect("write inner file");
        std::fs::write(base.join("outside.txt"), "outer").expect("write outer file");
        Self {
            _guard: guard,
            base,
            root,
        }
    }

    fn inner(&self) -> std::path::PathBuf {
        self.root.join("a.txt")
    }

    fn outside(&self) -> std::path::PathBuf {
        self.base.join("outside.txt")
    }
}

fn tool(name: &str) -> PortableDynamicTool {
    PortableDynamicTool::new(
        name,
        "资源声明测试壳工具",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" } }
            }
        }),
        |_args| Box::pin(async { Ok(ToolOutput::text("ok")) }),
    )
}

fn declared(name: &str, arguments: serde_json::Value, root: &std::path::Path) -> Vec<Claim> {
    declared_with_roots(name, arguments, root, &[])
}

fn declared_with_roots(
    name: &str,
    arguments: serde_json::Value,
    root: &std::path::Path,
    filesystem_roots: &[std::path::PathBuf],
) -> Vec<Claim> {
    let call = ToolCall::from_wire("call-1", ToolFunction::new(name.to_string(), arguments));
    claims(
        &tool(name),
        &call,
        "session-1",
        Some(root),
        filesystem_roots,
    )
}

#[test]
fn authorized_shared_path_gets_precise_claim_instead_of_session_fallback() {
    let workspace = Workspace::new();
    let shared = workspace.base.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    let file = shared.join("document.txt");
    std::fs::write(&file, "shared").unwrap();
    let claims = declared_with_roots(
        "read_file",
        serde_json::json!({ "path": file }),
        &workspace.root,
        &[shared],
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::File(path), write: false }] if path == &file
    ));
}

#[tokio::test]
async fn shells_in_distinct_sessions_serialize_on_any_shared_root() {
    let workspace = Workspace::new();
    let second_session = workspace.base.join("session-2");
    let shared = workspace.base.join("shared");
    let other_shared = workspace.base.join("other-shared");
    for directory in [&second_session, &shared, &other_shared] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let arbiter = Arc::new(ResourceArbiter::default());
    let (_tx, rx) = watch::channel(false);
    let deadline = Instant::now() + Duration::from_secs(60);
    let first = arbiter
        .acquire(
            declared_with_roots(
                "local_zsh",
                serde_json::json!({ "command": "pwd" }),
                &workspace.root,
                &[other_shared.clone(), shared.clone()],
            ),
            rx.clone(),
            deadline,
        )
        .await
        .unwrap();
    let second = arbiter.acquire(
        declared_with_roots(
            "local_zsh",
            serde_json::json!({ "command": "pwd" }),
            &second_session,
            &[shared],
        ),
        rx.clone(),
        deadline,
    );
    tokio::pin!(second);
    assert!(futures::poll!(&mut second).is_pending());
    let mcp = arbiter.acquire(
        declared_with_roots(
            "mcp__files__write",
            serde_json::json!({}),
            &second_session,
            &[other_shared],
        ),
        rx.clone(),
        deadline,
    );
    tokio::pin!(mcp);
    assert!(futures::poll!(&mut mcp).is_pending());
    // 同一个等待者不会挡住无关会话的独立文件操作。
    let independent = arbiter
        .acquire(
            vec![workspace_claim(&workspace.outside(), true)],
            rx,
            deadline,
        )
        .await
        .unwrap();
    drop(first);
    drop(second.await.unwrap());
    drop(mcp.await.unwrap());
    drop(independent);
}

#[test]
fn in_workspace_paths_keep_precise_readonly_file_claims() {
    let workspace = Workspace::new();
    for name in ["read_file", "list_dir"] {
        let claims = declared(
            name,
            serde_json::json!({ "path": "a.txt" }),
            &workspace.root,
        );
        assert!(
            matches!(
                &claims[..],
                [Claim { resource: Resource::File(path), write: false }]
                    if path == &workspace.inner()
            ),
            "{name} 应声明工作区内单文件只读锁"
        );
    }
}

#[test]
fn read_file_line_range_syntax_degrades_to_workspace_scope() {
    let workspace = Workspace::new();
    let claims = declared(
        "read_file",
        serde_json::json!({ "path": "a.txt:10-20" }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::LocalFilesystem(scope), write: false }]
            if scope == &workspace.root
    ));
}

#[test]
fn out_of_workspace_paths_degrade_to_workspace_scope() {
    let workspace = Workspace::new();
    let outside = workspace.outside().to_str().expect("utf8 path").to_string();
    let claims = declared(
        "list_dir",
        serde_json::json!({ "path": outside }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::LocalFilesystem(scope), write: false }]
            if scope == &workspace.root
    ));
}

#[test]
fn browser_and_canvas_tools_claim_session_write_lock() {
    let workspace = Workspace::new();
    for name in ["browser_open_url", "architecture_run"] {
        let claims = declared(name, serde_json::json!({}), &workspace.root);
        assert!(
            matches!(
                &claims[..],
                [Claim { resource: Resource::Session(session), write: true }]
                    if session == "session-1"
            ),
            "{name} 应声明会话级写锁"
        );
    }
}

#[test]
fn ssh_tools_claim_server_scoped_write_lock() {
    let workspace = Workspace::new();
    let claims = declared(
        "ssh_exec",
        serde_json::json!({ "server_id": "srv-1", "command": "ls" }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::Ssh(server), write: true }] if server == "srv-1"
    ));
    // 无 server_id 参数的枚举类按 unknown 服务器保守声明（与旧前缀推断一致）。
    let claims = declared("ssh_list_servers", serde_json::json!({}), &workspace.root);
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::Ssh(server), write: true }] if server == "unknown"
    ));
}

#[test]
fn sync_directory_claims_both_ssh_and_workspace_domains() {
    let workspace = Workspace::new();
    let claims = declared(
        "sync_directory",
        serde_json::json!({ "server_id": "srv-1" }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::Ssh(server), write: true },
         Claim { resource: Resource::LocalFilesystem(scope), write: true }]
            if server == "srv-1" && scope == &workspace.root
    ));
}

#[test]
fn unregistered_tools_fail_closed_to_external_write_lock() {
    let workspace = Workspace::new();
    // write_file 已随旧工具层删除：作为未登记名字必须走 External 写锁兜底。
    let claims = declared(
        "write_file",
        serde_json::json!({ "path": "a.txt" }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim {
            resource: Resource::External,
            write: true
        }]
    ));
}

#[test]
fn mixed_paths_keep_inner_claims_and_add_workspace_scope() {
    let workspace = Workspace::new();
    let inner = workspace.inner().to_str().expect("utf8 path").to_string();
    let outside = workspace.outside().to_str().expect("utf8 path").to_string();
    let claims = declared(
        "list_dir",
        serde_json::json!({ "paths": [inner, outside] }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::File(path), write: false },
         Claim { resource: Resource::LocalFilesystem(scope), write: false }]
            if path == &workspace.inner() && scope == &workspace.root
    ));
}

#[test]
fn workspace_bound_tools_declare_the_workspace_scope() {
    let workspace = Workspace::new();
    let claims = declared(
        "local_zsh",
        serde_json::json!({ "command": "ls" }),
        &workspace.root,
    );
    assert!(matches!(
        &claims[..],
        [Claim { resource: Resource::LocalFilesystem(scope), write: true }]
            if scope == &workspace.root
    ));
}
