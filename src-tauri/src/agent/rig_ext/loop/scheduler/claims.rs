//! 宿主资源声明；路径规范化只在 blocking 线程执行。
use super::*;
use crate::agent::rig_ext::tools::spec::ClaimResource;

/// 无宿主工作区时的兜底资源域（整机）：仅测试策略会走到，取最保守的独占范围。
const UNSCOPED_WORKSPACE: &str = "/";

/// 资源域用的工作区根：canonical 形态才能与 `File(..)` 声明（同为 canonical）做包含判定，
/// 也才能与工作流运行的工作区根写租约比对。解析失败退回词法归一化结果（仅作身份，不做包含判定）。
fn workspace_scope(root: &std::path::Path) -> std::path::PathBuf {
    crate::agent::rig_ext::tools::common::canonicalize_existing_prefix(root)
        .unwrap_or_else(|_| crate::agent::rig_ext::tools::common::lexical_normalize(root))
}

/// 工作区域级声明（带作用域：只在同一或嵌套工作区内互斥，跨项目会话不互相排队）。
fn workspace_claim(scope: &std::path::Path, write: bool) -> Claim {
    Claim {
        resource: Resource::LocalFilesystem(scope.to_path_buf()),
        write,
    }
}

pub(super) fn claims(
    tool: &PortableDynamicTool,
    call: &ToolCall,
    workspace: &str,
    root: Option<&std::path::Path>,
) -> Vec<Claim> {
    let name = tool.name();
    if !crate::agent::rig_ext::tools::spec::is_registered_tool_name(name) {
        return vec![Claim {
            resource: Resource::External,
            write: true,
        }];
    }
    // 资源域类型查策略表（唯一事实来源），不按名称前缀推断。
    let resource_kind = crate::agent::rig_ext::tools::spec::claim_resource(name);
    let scope = root
        .map(workspace_scope)
        .unwrap_or_else(|| std::path::PathBuf::from(UNSCOPED_WORKSPACE));
    // 文件路径域（read_file / list_dir / glob / grep）：按调用参数逐路径声明
    // 只读锁（FilePath 行必须为只读工具，由策略表一致性测试守护）。行范围
    // 语法或路径解析不确定时保守退化为工作区域级只读声明，不猜测可并行性。
    if resource_kind == ClaimResource::FilePath {
        if let Some(root) = root {
            let args = &call.function.arguments;
            let paths = args
                .get("paths")
                .and_then(serde_json::Value::as_array)
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    vec![args
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(".")]
                });
            let resolved = paths
                .iter()
                .map(|raw| {
                    if name == "read_file" && raw.contains(':') {
                        return None;
                    }
                    let path = std::path::Path::new(raw);
                    let joined = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        root.join(path)
                    };
                    let normalized =
                        crate::agent::rig_ext::tools::common::lexical_normalize(&joined);
                    crate::agent::rig_ext::tools::common::canonicalize_existing_prefix(&normalized)
                        .ok()
                })
                .collect::<Option<Vec<_>>>();
            if let Some(paths) = resolved.filter(|paths| !paths.is_empty()) {
                let mut leaves = Vec::with_capacity(paths.len());
                let mut out_of_scope = false;
                for path in paths {
                    // 越界路径（白名单外目录）不建独立锁——工具自身 resolve_path 才是
                    // 真实门禁，声明降级为工作区域级，避免无关工作区因同一越界路径排队。
                    if path.starts_with(&scope) {
                        leaves.push(Claim {
                            resource: Resource::File(path),
                            write: false,
                        });
                    } else {
                        out_of_scope = true;
                    }
                }
                if out_of_scope {
                    leaves.push(workspace_claim(&scope, false));
                }
                if !leaves.is_empty() {
                    return leaves;
                }
            }
        }
        return vec![workspace_claim(&scope, false)];
    }
    let resource = match resource_kind {
        ClaimResource::Session => Resource::Session(workspace.into()),
        ClaimResource::SshServer | ClaimResource::SshServerAndWorkspace => Resource::Ssh(
            call.function
                .arguments
                .get("server_id")
                .or_else(|| call.function.arguments.get("ssh_profile"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .into(),
        ),
        ClaimResource::Workspace => Resource::LocalFilesystem(scope.clone()),
        ClaimResource::External | ClaimResource::FilePath => Resource::External,
    };
    // Session / Ssh / External 即使工具只读也按写锁声明（共享会话、远端服务器与
    // 外部世界的观察与变更无法在声明层区分，保守串行）；Workspace 域按访问
    // 声明读写方向。
    let spec = ToolSpec::new(name, "", tool.definition().parameters);
    let write = !spec.access.readonly || resource_kind != ClaimResource::Workspace;
    let mut claims = vec![Claim { resource, write }];
    if resource_kind == ClaimResource::SshServerAndWorkspace {
        // sync_directory 同时改写远端目录与本地工作区文件树：双域都按写锁声明。
        claims.push(workspace_claim(&scope, true));
    }
    claims
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::ToolFunction;
    use rig::tool::ToolOutput;

    /// 临时工作区：`root` 为工作区，`base` 为其父目录（越界路径的落点）。
    struct Workspace {
        base: std::path::PathBuf,
        root: std::path::PathBuf,
    }

    impl Workspace {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!("rig-claims-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&base).expect("create temp root");
            let base = base.canonicalize().expect("canonicalize temp root");
            let root = base.join("ws");
            std::fs::create_dir_all(&root).expect("create workspace");
            std::fs::write(root.join("a.txt"), "inner").expect("write inner file");
            std::fs::write(base.join("outside.txt"), "outer").expect("write outer file");
            Self { base, root }
        }

        fn inner(&self) -> std::path::PathBuf {
            self.root.join("a.txt")
        }

        fn outside(&self) -> std::path::PathBuf {
            self.base.join("outside.txt")
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
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
        let call = ToolCall::from_wire("call-1", ToolFunction::new(name.to_string(), arguments));
        claims(&tool(name), &call, "session-1", Some(root))
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
        for name in ["browser_click", "browser_read_text", "architecture_run"] {
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
}
