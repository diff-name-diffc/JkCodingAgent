//! Git 命令的前端投影 DTO。
//!
//! 只承载序列化形态，不含查询逻辑；字段即前端契约，改动需同步前端类型。

#[derive(serde::Serialize)]
pub(crate) struct GitFileChange {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
    pub refs: Vec<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct GitBranchInfo {
    pub name: String,
    pub current: bool,
    pub remote: Option<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct GitCommitFile {
    pub path: String,
    pub status: String,
    pub additions: i32,
    pub deletions: i32,
}

#[derive(serde::Serialize)]
pub(crate) struct GitCommitDetail {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
    pub files: Vec<GitCommitFile>,
    pub total_additions: i32,
    pub total_deletions: i32,
}

#[derive(serde::Serialize)]
pub(crate) struct GitRemoteCounts {
    pub ahead: i32,
    pub behind: i32,
    pub branch: String,
}
