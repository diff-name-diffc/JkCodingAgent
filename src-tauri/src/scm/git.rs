//! SCM Git 集成。
//!
//! 本文件只保留模块错误类型与子模块声明；命令按变化原因拆分：
//! - `exec`：git 子进程执行管道与引用名校验（所有命令共用的底座）；
//! - `queries`：只读查询命令（状态/分支/日志/提交详情/远端计数）；
//! - `diffs`：diff 文本读取（提交整体/工作区文件/提交内单文件）；
//! - `mutations`：写操作命令（暂存/提交/分支切换/推送/拉取）；
//! - `commit_message`：基于项目对话模型的 AI 提交信息生成；
//! - `types`：前端投影 DTO（字段即前端契约）。
//!
//! 命令注册路径见 `app/mod.rs` 的 `invoke_handler!`。

use std::path::PathBuf;

pub(crate) mod commit_message;
pub(crate) mod diffs;
pub(crate) mod exec;
pub(crate) mod mutations;
pub(crate) mod queries;
pub(crate) mod types;

pub(crate) use exec::GitResult;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("分支名不能为空")]
    RefNameEmpty,
    #[error("分支名过长（{len} 字符），上限 256")]
    RefNameTooLong { len: usize },
    #[error("分支名 `{name}` 包含非法字符，仅允许字母、数字、/、-、_、.、@")]
    RefNameIllegalChars { name: String },
    #[error("分支名 `{name}` 包含非法路径遍历模式")]
    RefNameTraversal { name: String },
    #[error("执行命令失败（cwd={cwd}, args={args:?}）：{source}")]
    CommandIo {
        cwd: PathBuf,
        args: Vec<String>,
        #[source]
        source: std::io::Error,
    },
    #[error("Git 命令线程错误：{0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Git 命令执行超时（{secs}秒）")]
    Timeout { secs: u64 },
    #[error("Git 命令失败：{0}")]
    CommandFailed(String),
    #[error("没有可用于生成提交信息的已暂存变更。")]
    NoStagedChanges,
    #[error("生成提交信息超时（15秒）")]
    CommitMessageTimeout,
    #[error("智能体执行失败：{0}")]
    AgentFailed(String),
    #[error("智能体返回了空结果。")]
    EmptyAgentResult,
    #[error("读取项目配置失败：{0}")]
    ProjectConfig(String),
}
