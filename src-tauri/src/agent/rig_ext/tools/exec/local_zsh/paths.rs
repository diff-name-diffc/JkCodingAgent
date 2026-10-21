//! 执行目录与会话运行产物分开：切换工程 cwd 不改变审计和临时文件归属。
use super::{local_zsh_dir, ChatWorkspace, Path, PathBuf};
use crate::agent::rig_ext::tools::common::resolve_path;

pub(super) struct ExecutionPaths {
    pub run_dir: PathBuf,
    pub audit_dir: PathBuf,
    pub temp_dir: Option<PathBuf>,
}

pub(super) fn prepare(
    workspace: &Path,
    chat_workspace: Option<&ChatWorkspace>,
    working_directory: Option<&str>,
) -> Result<ExecutionPaths, String> {
    let Some(chat) = chat_workspace else {
        if working_directory.is_some() {
            return Err("错误：当前执行环境不支持 working_directory".into());
        }
        let run_dir = local_zsh_dir(workspace)?;
        std::fs::create_dir_all(&run_dir)
            .map_err(|error| format!("错误：创建 local_zsh 目录失败：{error}"))?;
        return Ok(ExecutionPaths {
            audit_dir: run_dir.clone(),
            run_dir,
            temp_dir: None,
        });
    };

    let run_dir = chat.validate_working_directory(working_directory)?;
    let audit_dir = resolve_path(
        &chat.session_root,
        true,
        &[],
        &local_zsh_dir(&chat.session_root)?.to_string_lossy(),
    )?;
    let temp_dir = resolve_path(&chat.session_root, true, &[], ".tmp")?;
    for directory in [&audit_dir, &temp_dir] {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "错误：创建会话运行目录 {} 失败：{error}",
                directory.display()
            )
        })?;
    }
    Ok(ExecutionPaths {
        run_dir,
        audit_dir,
        temp_dir: Some(temp_dir),
    })
}

#[cfg(test)]
mod tests;
