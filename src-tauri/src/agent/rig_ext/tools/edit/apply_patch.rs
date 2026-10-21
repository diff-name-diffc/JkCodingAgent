//! apply_patch 工具：把 apply-patch 格式补丁应用为本地文件编辑。
//!
//! 工具面契约参考 codex 的 `apply_patch`（核心编辑能力，模型侧事实标准），
//! 引擎见 `engine.rs`。本文件负责：参数校验、路径沙箱化、计算/提交两阶段
//! 应用与结果渲染。

use std::path::{Path, PathBuf};

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tokio::{
    task,
    time::{timeout, Duration},
};

use super::super::common::string_arg;
use super::super::fs::FsSandbox;
use super::engine::{self, Hunk};

/// 单次补丁输入的字符上限：拦住把整库塞进一个补丁的异常调用。
const MAX_PATCH_CHARS: usize = 200_000;
/// 计算阶段单文件读取上限（与 read_file 的单文件硬上限一致）。
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const APPLY_TIMEOUT_SECS: u64 = 30;

const DESCRIPTION: &str = "以补丁形式精确编辑文件：新增、修改、移动或删除。这是修改文件的首选工具（优先于 shell 重定向）。patch 使用 apply-patch 格式：\
`*** Begin Patch` 开头、`*** End Patch` 结尾；`*** Add File: <路径>` 后跟 `+` 开头的文件内容行；`*** Update File: <路径>` 后跟若干变更块（`@@ 可选定位上下文` 开头，` ` 前缀为上下文行、`-` 为要删除的行、`+` 为新行），可选 `*** Move to: <新路径>` 实现重命名；`*** Delete File: <路径>` 删除文件；`*** End of File` 标记块锚定文件末尾。\
修改已有文件时上下文与删除行必须与文件当前内容一致（容忍空白与常见 Unicode 标点差异），匹配失败不会改动任何文件；构造补丁前请先以当前可用的读取方式查看最新文件内容。一个补丁可包含多个文件的操作，全部计算成功后统一写入。同一路径在一个补丁中只能出现一次；Add File 目标必须不存在（覆盖请改用 Update File 或先 Delete File）；Move to 的目标也必须不存在。";

pub(super) fn apply_patch_tool(sandbox: FsSandbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "apply_patch",
        DESCRIPTION,
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "apply-patch 格式的补丁文本，以 *** Begin Patch 开头、*** End Patch 结尾。",
                    "minLength": 20,
                    "maxLength": 200000
                }
            },
            "required": ["patch"]
        }),
        move |args| {
            let sandbox = sandbox.clone();
            Box::pin(async move { execute(args, sandbox).await })
        },
    )
}

async fn execute(args: Value, sandbox: FsSandbox) -> Result<ToolOutput, ToolExecutionError> {
    let Some(patch) = string_arg(&args, "patch") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 patch（apply-patch 格式补丁文本）",
        ));
    };
    if patch.chars().count() > MAX_PATCH_CHARS {
        return Err(ToolExecutionError::invalid_args(format!(
            "错误：补丁超过 {MAX_PATCH_CHARS} 字符上限，请拆分为多个补丁分别应用"
        )));
    }

    let hunks =
        engine::parse_patch(&patch).map_err(|error| ToolExecutionError::invalid_args(
            format!("错误：补丁解析失败：{error}。补丁必须以 `*** Begin Patch` 开头、`*** End Patch` 结尾；Add File 块用 `+` 行给出内容；Update File 块用 `@@` 定位、` ` 上下文行、`-`/`+` 变更行。")
        ))?;
    if hunks.is_empty() {
        return Err(ToolExecutionError::invalid_args(
            "错误：补丁不包含任何文件操作块（Add/Update/Delete File），无事可做",
        ));
    }

    // 路径沙箱化在计算阶段完成：任何越界/敏感路径都在写盘前拒绝。
    let resolver_sandbox = sandbox.clone();
    let resolve = move |raw: &str| resolver_sandbox.resolve(raw);
    match timeout(
        Duration::from_secs(APPLY_TIMEOUT_SECS),
        apply_hunks_to_workspace(&hunks, &sandbox.workspace, resolve),
    )
    .await
    {
        Ok(Ok(summary)) => Ok(ToolOutput::text(render_summary(
            &summary,
            &sandbox.workspace,
        ))),
        Ok(Err(error)) => Err(commit_error(error)),
        Err(_) => Err(ToolExecutionError::timeout(format!(
            "错误：应用补丁超时（{APPLY_TIMEOUT_SECS}s）。写入任务可能仍在后台完成，结果未知：请重新读取目标文件确认实际状态后再决定是否重试"
        ))),
    }
}

/// 已提交的变更摘要（路径按工作区相对化展示）。
#[derive(Debug)]
pub(super) struct ApplySummary {
    pub(super) added: Vec<String>,
    pub(super) modified: Vec<String>,
    pub(super) deleted: Vec<String>,
}

enum PlannedChange {
    Add {
        path: PathBuf,
        contents: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        dest: PathBuf,
        /// move 时的目的地；非 move 与 path 相同。
        is_move: bool,
        new_contents: String,
    },
}

/// 计算阶段产物；任何一步失败直接返回错误，文件系统保持原样。
enum ComputedPatch {
    Success {
        changes: Vec<PlannedChange>,
        summary: ApplySummary,
    },
    Failure(String),
}

/// 把补丁块应用到工作区（先全量计算、后统一提交）。
///
/// `resolve` 把补丁里的原始路径解析为绝对路径（含沙箱校验）。测试里传
/// 直拼目录的解析器；生产路径来自 `FsSandbox::resolve`。要求 `'static`
/// 是因为计算/提交整体在 `spawn_blocking` 里执行。
pub(super) async fn apply_hunks_to_workspace(
    hunks: &[Hunk],
    workspace: &Path,
    resolve: impl Fn(&str) -> Result<PathBuf, String> + Send + Sync + 'static,
) -> Result<ApplySummary, String> {
    // spawn_blocking 外壳：文件 I/O 一律阻塞线程池。
    let hunks = hunks.to_vec();
    let workspace = workspace.to_path_buf();
    task::spawn_blocking(move || {
        let computed = compute_changes(&hunks, &workspace, &resolve);
        match computed {
            ComputedPatch::Success { changes, summary } => commit(changes).map(|()| summary),
            ComputedPatch::Failure(error) => Err(error),
        }
    })
    .await
    .map_err(|error| format!("错误：应用补丁任务失败：{error}"))?
}

fn compute_changes(
    hunks: &[Hunk],
    workspace: &Path,
    resolve: &dyn Fn(&str) -> Result<PathBuf, String>,
) -> ComputedPatch {
    let mut changes = Vec::new();
    let mut summary = ApplySummary {
        added: Vec::new(),
        modified: Vec::new(),
        deleted: Vec::new(),
    };
    // 同一补丁内所有受影响路径（Add/Delete 目标、Update 源与移动目的地）必须
    // 互不相同：各块独立从磁盘读原文推导，叠加到同一路径会互相覆盖或整批失败。
    let mut seen = std::collections::HashSet::new();

    for hunk in hunks {
        let label = hunk.path().to_string_lossy().to_string();
        match hunk {
            Hunk::AddFile { path, contents } => {
                let resolved = match resolve(&path.to_string_lossy()) {
                    Ok(resolved) => resolved,
                    Err(error) => return ComputedPatch::Failure(error),
                };
                if !seen.insert(resolved.clone()) {
                    return ComputedPatch::Failure(duplicate_target(&label, workspace));
                }
                // Add 语义是新建：已存在的目标必须显式走 Update（或先 Delete），
                // 避免误用静默截断既有内容（本项目没有 codex 的回滚台账）。
                if resolved.exists() {
                    return ComputedPatch::Failure(format!(
                        "错误：Add File 目标已存在：{}。如需修改请改用 Update File，或先 Delete File 再 Add",
                        display_path(&label, workspace)
                    ));
                }
                changes.push(PlannedChange::Add {
                    path: resolved,
                    contents: contents.clone(),
                });
                summary.added.push(display_path(&label, workspace));
            }
            Hunk::DeleteFile { path } => {
                let resolved = match resolve(&path.to_string_lossy()) {
                    Ok(resolved) => resolved,
                    Err(error) => return ComputedPatch::Failure(error),
                };
                if !seen.insert(resolved.clone()) {
                    return ComputedPatch::Failure(duplicate_target(&label, workspace));
                }
                changes.push(PlannedChange::Delete { path: resolved });
                summary.deleted.push(display_path(&label, workspace));
            }
            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let resolved = match resolve(&path.to_string_lossy()) {
                    Ok(resolved) => resolved,
                    Err(error) => return ComputedPatch::Failure(error),
                };
                if !seen.insert(resolved.clone()) {
                    return ComputedPatch::Failure(duplicate_target(&label, workspace));
                }
                let dest = match move_path {
                    Some(dest) => match resolve(&dest.to_string_lossy()) {
                        Ok(resolved) => resolved,
                        Err(error) => return ComputedPatch::Failure(error),
                    },
                    None => resolved.clone(),
                };
                // move 到自身按普通更新处理：否则会先写 dest 再删 path，把文件删掉。
                let is_move = move_path.is_some() && dest != resolved;
                if is_move {
                    if !seen.insert(dest.clone()) {
                        return ComputedPatch::Failure(duplicate_target(&label, workspace));
                    }
                    if dest.exists() {
                        return ComputedPatch::Failure(format!(
                            "错误：Move to 目标已存在：{}。如需覆盖请先 Delete File",
                            display_path(&label, workspace)
                        ));
                    }
                }
                let original = match read_text_capped(&resolved) {
                    Ok(original) => original,
                    Err(error) => return ComputedPatch::Failure(error),
                };
                // 匹配失败的报错标签用源路径：需要重读修正的是尚存在的源文件，
                // 而非移动目的地。
                let source_label = path.to_string_lossy().to_string();
                let new_contents =
                    match engine::derive_new_contents(&original, &source_label, chunks) {
                        Ok(contents) => contents,
                        Err(error) => return ComputedPatch::Failure(format!("错误：{error}")),
                    };
                changes.push(PlannedChange::Update {
                    path: resolved,
                    dest,
                    is_move,
                    new_contents,
                });
                summary.modified.push(display_path(&label, workspace));
            }
        }
    }

    ComputedPatch::Success { changes, summary }
}

fn duplicate_target(label: &str, workspace: &Path) -> String {
    format!(
        "错误：同一补丁多次操作同一路径：{}。请拆分为多个补丁分别应用",
        display_path(label, workspace)
    )
}

fn commit(changes: Vec<PlannedChange>) -> Result<(), String> {
    let mut committed: Vec<String> = Vec::new();
    let result = commit_inner(&changes, &mut committed);
    if let Err(error) = &result {
        if !committed.is_empty() {
            return Err(format!(
                "{error}\n注意：以下文件已在失败前写入：{}\n（其余文件未改动）",
                committed.join("、")
            ));
        }
    }
    result
}

fn commit_inner(changes: &[PlannedChange], committed: &mut Vec<String>) -> Result<(), String> {
    for change in changes {
        match change {
            PlannedChange::Add { path, contents, .. } => {
                write_with_parents(path, contents).map_err(|error| {
                    format!("错误：Failed to write file {}: {error}", path.display())
                })?;
                committed.push(path.display().to_string());
            }
            PlannedChange::Delete { path } => {
                if std::fs::symlink_metadata(path)
                    .map(|meta| meta.is_dir())
                    .unwrap_or(false)
                {
                    return Err(format!(
                        "错误：Failed to delete file {}：路径是目录，仅支持删除文件",
                        path.display()
                    ));
                }
                std::fs::remove_file(path).map_err(|error| {
                    format!("错误：Failed to delete file {}: {error}", path.display())
                })?;
                committed.push(path.display().to_string());
            }
            PlannedChange::Update {
                path,
                dest,
                is_move,
                new_contents,
            } => {
                write_with_parents(dest, new_contents).map_err(|error| {
                    format!("错误：Failed to write file {}: {error}", dest.display())
                })?;
                committed.push(dest.display().to_string());
                if *is_move {
                    std::fs::remove_file(path).map_err(|error| {
                        format!(
                            "错误：Failed to remove original {}: {error}",
                            path.display()
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn write_with_parents(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

fn read_text_capped(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        format!(
            "错误：Failed to read file to update {}：{error}",
            path.display()
        )
    })?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "错误：要修改的文件 {} 超过 2MB 上限，apply_patch 不支持该规模的文件",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "错误：Failed to read file to update {}：{error}",
            path.display()
        )
    })?;
    String::from_utf8(bytes).map_err(|_| {
        format!(
            "错误：要修改的文件 {} 不是 UTF-8 文本，apply_patch 仅支持文本文件",
            path.display()
        )
    })
}

fn display_path(label: &str, workspace: &Path) -> String {
    let path = Path::new(label);
    path.strip_prefix(workspace)
        .map(|rel| rel.display().to_string())
        .unwrap_or_else(|_| label.to_string())
}

fn render_summary(summary: &ApplySummary, workspace: &Path) -> String {
    let mut out = String::from("Success. Updated the following files:\n");
    for path in &summary.added {
        out.push_str(&format!("A {}\n", display_path(path, workspace)));
    }
    for path in &summary.modified {
        out.push_str(&format!("M {}\n", display_path(path, workspace)));
    }
    for path in &summary.deleted {
        out.push_str(&format!("D {}\n", display_path(path, workspace)));
    }
    out
}

fn commit_error(message: String) -> ToolExecutionError {
    // 计算期失败（内容不匹配/路径越界）模型可通过重读文件修正补丁后重试；
    // 提交期 I/O 失败重试通常无效。
    if message.starts_with("错误：Failed to find") || message.starts_with("错误：Failed to read")
    {
        ToolExecutionError::other(message)
    } else {
        ToolExecutionError::other(message).with_retryable(false)
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
