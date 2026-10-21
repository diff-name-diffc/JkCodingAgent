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

fn workspace_claims(scopes: &[std::path::PathBuf], write: bool) -> Vec<Claim> {
    scopes
        .iter()
        .map(|scope| workspace_claim(scope, write))
        .collect()
}

/// 解析 Ssh 资源键：server_id / ssh_profile 参数优先，其次 term_id 反查，
/// 兜底 "unknown"。
fn ssh_resource_key(args: &serde_json::Value) -> String {
    if let Some(key) = args
        .get("server_id")
        .or_else(|| args.get("ssh_profile"))
        .and_then(serde_json::Value::as_str)
    {
        return key.to_string();
    }
    if let Some(term_id) = args.get("term_id").and_then(serde_json::Value::as_str) {
        if let Some(server_id) = crate::ssh_tool::term::global_registry()
            .and_then(|registry| registry.server_of(term_id))
        {
            return server_id;
        }
    }
    "unknown".to_string()
}

pub(super) fn claims(
    tool: &PortableDynamicTool,
    call: &ToolCall,
    workspace: &str,
    root: Option<&std::path::Path>,
    filesystem_roots: &[std::path::PathBuf],
) -> Vec<Claim> {
    let name = tool.name();
    let scope = root
        .map(workspace_scope)
        .unwrap_or_else(|| std::path::PathBuf::from(UNSCOPED_WORKSPACE));
    let mut scopes = vec![scope];
    for root in filesystem_roots {
        let scope = workspace_scope(root);
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    if !crate::agent::rig_ext::tools::spec::is_registered_tool_name(name) {
        let mut claims = vec![Claim {
            resource: Resource::External,
            write: true,
        }];
        if !filesystem_roots.is_empty() {
            // MCP 可封装本地写入；仅锁 External 无法与共享目录里的 shell 互斥。
            claims.extend(workspace_claims(&scopes, true));
        }
        return claims;
    }
    // 资源域类型查策略表（唯一事实来源），不按名称前缀推断。
    let resource_kind = crate::agent::rig_ext::tools::spec::claim_resource(name);
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
                    if scopes.iter().any(|scope| path.starts_with(scope)) {
                        leaves.push(Claim {
                            resource: Resource::File(path),
                            write: false,
                        });
                    } else {
                        out_of_scope = true;
                    }
                }
                if out_of_scope {
                    leaves.extend(workspace_claims(&scopes, false));
                }
                if !leaves.is_empty() {
                    return leaves;
                }
            }
        }
        return workspace_claims(&scopes, false);
    }
    let spec = ToolSpec::new(name, "", tool.definition().parameters);
    if resource_kind == ClaimResource::Workspace {
        // Shell 可通过绝对路径访问任一许可根；不能只锁当前 cwd。
        return workspace_claims(&scopes, !spec.access.readonly);
    }
    let resource = match resource_kind {
        ClaimResource::Session => Resource::Session(workspace.into()),
        // ssh_term_send/read/close 只带 term_id：经全局终端会话表反查
        // server_id，保证与同 server 的 ssh_exec 等副作用互斥；查不到（已
        // 关闭/回收/未登记）保守落 "unknown" 域。
        ClaimResource::SshServer | ClaimResource::SshServerAndWorkspace => {
            Resource::Ssh(ssh_resource_key(&call.function.arguments))
        }
        ClaimResource::Workspace => {
            // 上方已按全部许可根提前返回；防御性兜底，分支顺序调整不得 panic。
            return workspace_claims(&scopes, true);
        }
        ClaimResource::External | ClaimResource::FilePath => Resource::External,
    };
    // Session / Ssh / External 的观察与变更无法在声明层区分，保守串行。
    let mut claims = vec![Claim {
        resource,
        write: true,
    }];
    if resource_kind == ClaimResource::SshServerAndWorkspace {
        // sync_directory 同时改写远端目录与本地工作区文件树：双域都按写锁声明。
        claims.extend(workspace_claims(&scopes, true));
    } else if resource_kind == ClaimResource::External
        && spec.access.mutates_filesystem
        && !filesystem_roots.is_empty()
    {
        // 仅实际会写本地文件系统的外部工具才锁许可根：纯网络/子系统类
        // （generate_image / analyze_image / call_sub_agent 等）执行期长，
        // 全根写锁会把其它会话的共享目录操作排队到超时。未登记工具（MCP
        // 等）的兜底锁在上方 fail-closed 分支覆盖。
        claims.extend(workspace_claims(&scopes, true));
    }
    claims
}

#[cfg(test)]
mod tests;
