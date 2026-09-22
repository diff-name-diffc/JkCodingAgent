use std::path::{Path, PathBuf};

pub(crate) fn local_zsh_dir(workspace: &Path) -> Result<PathBuf, String> {
    // plain-chat-browser 是聊天占位工作区，其执行目录按设计落在
    // workspace.parent()/local_env/zsh；此时以 workspace.parent() 为合法根目录做
    // 包含性校验。其余情况执行目录必须位于工作区根目录内。
    let is_plain_chat = workspace
        .file_name()
        .is_some_and(|name| name == "plain-chat-browser");
    let allowed_root = if is_plain_chat {
        workspace
            .parent()
            .ok_or_else(|| "错误：无法解析聊天工作区根目录".to_string())?
            .to_path_buf()
    } else {
        workspace.to_path_buf()
    };
    let dir = if is_plain_chat {
        allowed_root.join("local_env").join("zsh")
    } else {
        workspace
            .join(".jkcodingagent")
            .join("local_env")
            .join("zsh")
    };
    let normalized = super::common::lexical_normalize(&dir);
    let normalized_root = super::common::lexical_normalize(&allowed_root);
    if !normalized.starts_with(&normalized_root) {
        return Err("错误：local_zsh 目录解析到了合法根目录之外".to_string());
    }
    Ok(normalized)
}
