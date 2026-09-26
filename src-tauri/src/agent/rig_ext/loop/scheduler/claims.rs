//! 宿主资源声明；路径规范化只在 blocking 线程执行。
use super::*;

/// 无宿主工作区时的兜底资源域（整机）：仅测试策略会走到，取最保守的独占范围。
const UNSCOPED_WORKSPACE: &str = "/";

/// 资源域用的工作区根：canonical 形态才能与 `File(..)` 声明（同为 canonical）做包含判定，
/// 也才能与图运行的工作区根写租约比对。解析失败退回词法归一化结果（仅作身份，不做包含判定）。
fn workspace_scope(root: &std::path::Path) -> std::path::PathBuf {
    crate::agent::rig_ext::tools::common::canonicalize_existing_prefix(root)
        .unwrap_or_else(|_| crate::agent::rig_ext::tools::common::lexical_normalize(root))
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
    let spec = ToolSpec::new(name, "", tool.definition().parameters);
    let scope = root
        .map(workspace_scope)
        .unwrap_or_else(|| std::path::PathBuf::from(UNSCOPED_WORKSPACE));
    if matches!(
        name,
        "read_file" | "write_file" | "edit_file" | "list_dir" | "glob" | "grep"
    ) {
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
            // 行范围语法或路径解析不确定时保守占整个文件域，不猜测可并行性。
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
                let write = !spec.access.readonly;
                let mut leaves = Vec::with_capacity(paths.len());
                let mut out_of_scope = false;
                for path in paths {
                    // 越界路径（白名单外目录）不建独立锁——工具自身 resolve_path 才是
                    // 真实门禁，声明降级为工作区域级，避免无关工作区因同一越界路径排队。
                    if path.starts_with(&scope) {
                        leaves.push(Claim {
                            resource: Resource::File(path),
                            write,
                        });
                    } else {
                        out_of_scope = true;
                    }
                }
                if out_of_scope {
                    leaves.push(Claim {
                        resource: Resource::LocalFilesystem(scope.clone()),
                        write,
                    });
                }
                if !leaves.is_empty() {
                    return leaves;
                }
            }
        }
    }
    let resource = if name.starts_with("browser_") || name == "architecture_run" {
        Resource::Session(workspace.into())
    } else if name.starts_with("ssh_") || name == "sync_directory" {
        Resource::Ssh(
            call.function
                .arguments
                .get("server_id")
                .or_else(|| call.function.arguments.get("ssh_profile"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .into(),
        )
    } else if spec.access.workspace_bound || name == "local_zsh" {
        // 工作区域级声明带作用域：只在同一或嵌套工作区内互斥，跨项目会话不再互相排队。
        Resource::LocalFilesystem(scope.clone())
    } else {
        Resource::External
    };
    let write = !spec.access.readonly
        || matches!(
            resource,
            Resource::Session(_) | Resource::Ssh(_) | Resource::External
        );
    let mut claims = vec![Claim { resource, write }];
    if name == "sync_directory" {
        claims.push(Claim {
            resource: Resource::LocalFilesystem(scope),
            write: true,
        });
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
    fn in_workspace_paths_keep_precise_file_claims() {
        let workspace = Workspace::new();
        let claims = declared(
            "write_file",
            serde_json::json!({ "path": "a.txt" }),
            &workspace.root,
        );
        assert!(matches!(
            &claims[..],
            [Claim { resource: Resource::File(path), write: true }]
                if path == &workspace.inner()
        ));
        let claims = declared(
            "list_dir",
            serde_json::json!({ "path": "a.txt" }),
            &workspace.root,
        );
        assert!(matches!(
            &claims[..],
            [Claim { resource: Resource::File(path), write: false }]
                if path == &workspace.inner()
        ));
    }

    #[test]
    fn out_of_workspace_paths_degrade_to_workspace_scope() {
        let workspace = Workspace::new();
        let outside = workspace.outside().to_str().expect("utf8 path").to_string();
        let claims = declared(
            "write_file",
            serde_json::json!({ "path": outside }),
            &workspace.root,
        );
        assert!(matches!(
            &claims[..],
            [Claim { resource: Resource::LocalFilesystem(scope), write: true }]
                if scope == &workspace.root
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
