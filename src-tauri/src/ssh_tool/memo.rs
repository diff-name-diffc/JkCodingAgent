//! SSH 服务器运维备忘录存储。
//!
//! 每台服务器一份 Markdown 文件：`~/.jkcodingagent/ssh-memos/{server_id}.md`，
//! 记录对该服务器运维长期有价值的必要信息（部署/服务路径、非通用命令与
//! 操作方式、已知问题与解法等）。消费方：
//! - Agent 工具 `ssh_memo_read` / `ssh_memo_upsert` / `ssh_memo_delete`
//!   （`agent/tools/builtin/ssh_memo.rs`）；
//! - 设置页命令 `ssh_tool_get_memo` / `ssh_tool_save_memo`（`commands.rs`）。
//!
//! 防无限增长（fail-closed）：全文 ≤ [`MEMO_MAX_CHARS`] 字符、单段 ≤
//! [`SECTION_MAX_CHARS`] 字符；任何写入先计算写入后的全文长度，超限即拒绝
//! 落盘并返回各段长度清单，由调用方（Agent/用户）先精简合并再重试。
//!
//! 并发与原子性（照 `local_zsh/audit.rs` 的模式）：按 server_id 粒度写锁
//! 串行化同一文件的读-改-写——持锁期间仅做该备忘录文件的 I/O；落盘用
//! 临时文件 + rename 原子写，避免写一半崩溃留下损坏文件。
//!
//! 路径安全：`server_id` 直接拼进文件路径，必须先过 `validate_server_id`
//! 白名单（小写字母/数字/-/_，≤64 字符），这是目录穿越的唯一闸门。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::validation::validate_server_id;
use super::SshMemoPayload;

/// 备忘录目录名（`~/.jkcodingagent/` 下）。
pub(crate) const MEMO_DIR_NAME: &str = "ssh-memos";

/// 全文字符上限（Unicode 标量计数）。同时保证 `ssh_memo_read` 的输出
/// 落在读取类工具 10000 字符内联档内（见 `common/tool_result.rs`）。
pub(crate) const MEMO_MAX_CHARS: usize = 8000;

/// 单段字符上限。
pub(crate) const SECTION_MAX_CHARS: usize = 4000;

/// 段落标题长度上限。
pub(crate) const TITLE_MAX_CHARS: usize = 40;

/// 工具生成的头部说明注释的识别前缀（解析时剥离、写盘时重新生成）。
const GENERATED_HEADER_PREFIX: &str = "<!-- 运维备忘录";

fn generated_header() -> String {
    format!(
        "{GENERATED_HEADER_PREFIX}：由 ssh_memo_* 工具与设置页维护；全文上限 {MEMO_MAX_CHARS} 字符、单段上限 {SECTION_MAX_CHARS} 字符，超限写入会被拒绝。只记录部署路径、特殊命令方式、已知问题与解法等必要信息，禁止记录凭据。 -->"
    )
}

/// 备忘录的一个 `## <标题>` 段落。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoSection {
    pub title: String,
    /// 段落正文（不含标题行；首尾空白已修剪）。
    pub body: String,
}

/// 备忘录解析结果：头部注释之外的自由前言 + 段落列表（保持文件内顺序）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedMemo {
    /// 首个段落之前的自由文本（用户手写内容），重写时原样保留。
    pub preamble: String,
    pub sections: Vec<MemoSection>,
}

/// upsert 写入结果（供工具层组织成功消息）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoWriteOutcome {
    /// true=替换了同名已有段落；false=新增段落。
    pub replaced: bool,
    /// 写后全文字符数。
    pub total_chars: usize,
}

// ─── 路径 ───

/// 备忘录文件路径：`~/.jkcodingagent/ssh-memos/{server_id}.md`。
/// `server_id` 先过白名单校验（目录穿越唯一闸门）。
pub(crate) fn memo_path(server_id: &str) -> Result<PathBuf, String> {
    let home = crate::agent::config::resolve_home_dir()
        .map_err(|error| format!("解析用户主目录失败：{error}"))?;
    memo_path_in(&home, server_id)
}

/// 纯路径拼接（可单测）：`root/ssh-memos/{server_id}.md`。
pub(crate) fn memo_path_in(root: &Path, server_id: &str) -> Result<PathBuf, String> {
    validate_server_id(server_id).map_err(|error| format!("非法 server_id：{error}"))?;
    Ok(root
        .join(MEMO_DIR_NAME)
        .join(format!("{server_id}.md")))
}

// ─── 读 ───

/// 读取备忘录原文（含头部注释）。文件不存在返回空内容（成功语义）。
pub(crate) fn read_memo(server_id: &str) -> Result<SshMemoPayload, String> {
    let path = memo_path(server_id)?;
    read_memo_at(&path)
}

fn read_memo_at(path: &Path) -> Result<SshMemoPayload, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(SshMemoPayload {
            content,
            updated_at: file_mtime_rfc3339(path),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SshMemoPayload {
            content: String::new(),
            updated_at: None,
        }),
        Err(error) => Err(format!("读取备忘录失败：{error}")),
    }
}

/// 备忘录字符数（文件不存在或不可读为 0）。供 `ssh_list_servers` 投影。
pub(crate) fn memo_char_count(server_id: &str) -> usize {
    memo_path(server_id)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|content| content.chars().count())
        .unwrap_or(0)
}

// ─── 写：设置页整文保存 ───

/// 设置页人工保存：整文替换。内容做与 Agent 工具相同的上限校验并规范化
/// （剥离旧头部注释、重新生成、段落格式归一）；清空内容视为删除备忘录。
pub(crate) fn save_memo_full(server_id: &str, content: &str) -> Result<SshMemoPayload, String> {
    let path = memo_path(server_id)?;
    // 锁的用途就是串行化本文件的读-改-写，持锁期间仅做该备忘录文件的 I/O。
    let lock = memo_lock_for(server_id);
    let _guard = lock.lock();
    if content.trim().is_empty() {
        delete_file_at(&path)?;
        return Ok(SshMemoPayload {
            content: String::new(),
            updated_at: None,
        });
    }
    let parsed = parse_sections(content);
    // 段落标题与 Agent 写路径同一约束（非空、≤ TITLE_MAX_CHARS）：否则设置页
    // 存入的段落会被 ssh_memo_upsert/delete 的 validate_memo_title 拒绝，
    // 智能体无法定位与维护该段。
    for section in &parsed.sections {
        validate_memo_title(&section.title).map_err(|error| format!("段落标题校验失败：{error}"))?;
    }
    let rendered = render_memo(&parsed);
    check_limits(&rendered, &parsed)?;
    atomic_write_at(&path, &rendered)?;
    Ok(SshMemoPayload {
        content: rendered,
        updated_at: Some(Utc::now().to_rfc3339()),
    })
}

// ─── 写：段落级更新（Agent 工具语义）───

/// 按段标题原位替换整段（不存在则追加到末尾）。超限拒绝写盘。
pub(crate) fn upsert_section(
    server_id: &str,
    title: &str,
    body: &str,
) -> Result<MemoWriteOutcome, String> {
    let title = validate_memo_title(title)?;
    let body = normalize_body(body);
    if body.chars().count() > SECTION_MAX_CHARS {
        return Err(format!(
            "段落「{title}」内容 {} 字符，超过单段上限 {SECTION_MAX_CHARS} 字符；请精简该段后再写入",
            body.chars().count()
        ));
    }
    let path = memo_path(server_id)?;
    let lock = memo_lock_for(server_id);
    let _guard = lock.lock();
    let mut parsed = parse_sections(&read_content_at(&path)?);
    let replaced = apply_upsert(&mut parsed, &title, &body);
    let rendered = render_memo(&parsed);
    check_limits(&rendered, &parsed)?;
    atomic_write_at(&path, &rendered)?;
    Ok(MemoWriteOutcome {
        replaced,
        total_chars: rendered.chars().count(),
    })
}

/// 删除整段。返回 false 表示备忘录中不存在该段（未做任何写入）。
pub(crate) fn remove_section(server_id: &str, title: &str) -> Result<bool, String> {
    let title = validate_memo_title(title)?;
    let path = memo_path(server_id)?;
    let lock = memo_lock_for(server_id);
    let _guard = lock.lock();
    let mut parsed = parse_sections(&read_content_at(&path)?);
    if !apply_remove(&mut parsed, &title) {
        return Ok(false);
    }
    let rendered = render_memo(&parsed);
    atomic_write_at(&path, &rendered)?;
    Ok(true)
}

/// 删除备忘录文件（服务器删除时的级联清理）。幂等：文件不存在视为成功。
pub(crate) fn delete_memo_file(server_id: &str) -> Result<(), String> {
    let path = memo_path(server_id)?;
    let lock = memo_lock_for(server_id);
    let _guard = lock.lock();
    delete_file_at(&path)
}

// ─── 纯函数：解析 / 渲染 / 段落操作 / 校验 ───

/// 按行首 `## ` 切段。工具生成的头部注释被剥离；首个段落前的其余自由
/// 文本进 `preamble`（重写时保留）。段落正文与前言的首尾空白被修剪。
pub(crate) fn parse_sections(content: &str) -> ParsedMemo {
    let mut preamble_lines: Vec<&str> = Vec::new();
    let mut sections: Vec<MemoSection> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in content.split('\n') {
        if let Some(raw_title) = line.strip_prefix("## ") {
            if let Some((title, body)) = current.take() {
                sections.push(MemoSection {
                    title,
                    body: join_trim(&body),
                });
            }
            current = Some((raw_title.trim().to_string(), Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        } else if !line.trim_start().starts_with(GENERATED_HEADER_PREFIX) {
            preamble_lines.push(line);
        }
    }
    if let Some((title, body)) = current.take() {
        sections.push(MemoSection {
            title,
            body: join_trim(&body),
        });
    }
    ParsedMemo {
        preamble: join_trim(&preamble_lines),
        sections,
    }
}

/// 渲染整文：头部注释 + 前言 + 各段。与 [`parse_sections`] 构成稳定往返
/// （render(parse(render(parse(x)))) == render(parse(x))）。
pub(crate) fn render_memo(parsed: &ParsedMemo) -> String {
    let mut out = generated_header();
    out.push('\n');
    if !parsed.preamble.is_empty() {
        out.push('\n');
        out.push_str(&parsed.preamble);
        out.push('\n');
    }
    for section in &parsed.sections {
        out.push('\n');
        out.push_str("## ");
        out.push_str(&section.title);
        out.push('\n');
        if !section.body.is_empty() {
            out.push('\n');
            out.push_str(&section.body);
            out.push('\n');
        }
    }
    out
}

/// 原位替换同名段落，不存在则追加到末尾。返回是否替换。
pub(crate) fn apply_upsert(parsed: &mut ParsedMemo, title: &str, body: &str) -> bool {
    if let Some(section) = parsed
        .sections
        .iter_mut()
        .find(|section| section.title == title)
    {
        section.body = body.to_string();
        return true;
    }
    parsed.sections.push(MemoSection {
        title: title.to_string(),
        body: body.to_string(),
    });
    false
}

/// 删除同名段落。返回是否删除。
pub(crate) fn apply_remove(parsed: &mut ParsedMemo, title: &str) -> bool {
    let before = parsed.sections.len();
    parsed.sections.retain(|section| section.title != title);
    parsed.sections.len() < before
}

/// 段落标题校验并规范化：非空、trim、剥离前导 `#`、≤ [`TITLE_MAX_CHARS`]
/// 字符、不含换行。返回规范化后的标题。
pub(crate) fn validate_memo_title(title: &str) -> Result<String, String> {
    let normalized = title.trim().trim_start_matches('#').trim();
    if normalized.is_empty() {
        return Err("段落标题（title）不能为空".to_string());
    }
    if normalized.contains('\n') || normalized.contains('\r') {
        return Err("段落标题（title）不能包含换行".to_string());
    }
    if normalized.chars().count() > TITLE_MAX_CHARS {
        return Err(format!(
            "段落标题（title）不能超过 {TITLE_MAX_CHARS} 个字符"
        ));
    }
    Ok(normalized.to_string())
}

/// 全文 + 单段上限校验（fail-closed，超限拒绝写盘）。
/// `rendered` 是即将写入的完整文件内容。
fn check_limits(rendered: &str, parsed: &ParsedMemo) -> Result<(), String> {
    for section in &parsed.sections {
        let section_chars = section.body.chars().count();
        if section_chars > SECTION_MAX_CHARS {
            return Err(format!(
                "段落「{}」{} 字符，超过单段上限 {SECTION_MAX_CHARS} 字符；请精简该段",
                section.title, section_chars
            ));
        }
    }
    let total = rendered.chars().count();
    if total > MEMO_MAX_CHARS {
        return Err(limit_exceeded_message(total, parsed));
    }
    Ok(())
}

fn limit_exceeded_message(total: usize, parsed: &ParsedMemo) -> String {
    let listing = parsed
        .sections
        .iter()
        .map(|section| format!("{}: {}", section.title, section.body.chars().count()))
        .collect::<Vec<_>>()
        .join("；");
    format!(
        "写入后全文 {total} 字符，超过备忘录上限 {MEMO_MAX_CHARS} 字符，已拒绝写入。当前各段字符数——{listing}。请先合并重复条目、删除过时内容（ssh_memo_read 后整段重写）再重试。"
    )
}

/// 段落正文规范化：首尾修剪，与 `parse_sections` 的 `join_trim` 语义一致，
/// 保证 render→parse 往返稳定。
fn normalize_body(body: &str) -> String {
    body.trim().to_string()
}

fn join_trim(lines: &[&str]) -> String {
    lines.join("\n").trim().to_string()
}

// ─── 文件 I/O 基础设施 ───

/// 按 server_id 粒度的写锁：并发调用串行化同一备忘录文件的读-改-写。
fn memo_lock_for(server_id: &str) -> Arc<Mutex<()>> {
    static MEMO_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    MEMO_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .entry(server_id.to_string())
        .or_default()
        .clone()
}

fn read_content_at(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("读取备忘录失败：{error}")),
    }
}

/// 原子写：先写同目录临时文件再 rename，避免写一半崩溃留下半个文件。
fn atomic_write_at(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建备忘录目录失败：{error}"))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "备忘录文件名非法".to_string())?;
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp"));
    fs::write(&tmp_path, content).map_err(|error| format!("写入备忘录失败：{error}"))?;
    fs::rename(&tmp_path, path).map_err(|error| format!("写入备忘录失败：{error}"))
}

fn delete_file_at(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除备忘录文件失败：{error}")),
    }
}

fn file_mtime_rfc3339(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some(DateTime::<Utc>::from(modified).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aha-ssh-memo-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn memo_path_rejects_traversal_and_bad_ids() {
        let root = PathBuf::from("/tmp/jk-memo-test");
        assert!(memo_path_in(&root, "../evil").is_err());
        assert!(memo_path_in(&root, "a/b").is_err());
        assert!(memo_path_in(&root, "").is_err());
        assert!(memo_path_in(&root, "UPPER").is_err());
        let ok = memo_path_in(&root, "prod-web_01").unwrap();
        assert_eq!(
            ok,
            PathBuf::from("/tmp/jk-memo-test/ssh-memos/prod-web_01.md")
        );
    }

    #[test]
    fn parse_splits_sections_and_strips_generated_header() {
        let content = format!(
            "{}\n\n手写前言第一行\n手写前言第二行\n\n## 部署路径\n\n- /opt/app\n\n## 特殊命令\n- systemctl restart app --legacy\n## 已知问题与解法\n\n### 连接池耗尽\n处置：调大 maxPoolSize\n",
            generated_header()
        );
        let parsed = parse_sections(&content);
        assert_eq!(parsed.preamble, "手写前言第一行\n手写前言第二行");
        assert_eq!(parsed.sections.len(), 3);
        assert_eq!(parsed.sections[0].title, "部署路径");
        assert_eq!(parsed.sections[0].body, "- /opt/app");
        assert_eq!(parsed.sections[1].body, "- systemctl restart app --legacy");
        assert_eq!(
            parsed.sections[2].body,
            "### 连接池耗尽\n处置：调大 maxPoolSize"
        );
    }

    #[test]
    fn parse_of_empty_and_header_only_is_empty() {
        let parsed = parse_sections("");
        assert_eq!(parsed.preamble, "");
        assert!(parsed.sections.is_empty());
        let parsed = parse_sections(&format!("{}\n", generated_header()));
        assert_eq!(parsed.preamble, "");
        assert!(parsed.sections.is_empty());
    }

    #[test]
    fn render_parse_roundtrip_is_stable() {
        let mut parsed = ParsedMemo {
            preamble: "自由前言".to_string(),
            ..Default::default()
        };
        apply_upsert(&mut parsed, "部署路径", "- /opt/app");
        apply_upsert(&mut parsed, "特殊命令", "- cmd a\n- cmd b");
        let once = render_memo(&parsed);
        let twice = render_memo(&parse_sections(&once));
        assert_eq!(once, twice);
        let reparsed = parse_sections(&once);
        assert_eq!(reparsed.preamble, "自由前言");
        assert_eq!(reparsed.sections.len(), 2);
        assert_eq!(reparsed.sections[1].title, "特殊命令");
    }

    #[test]
    fn apply_upsert_replaces_in_place_and_keeps_order() {
        let mut parsed = ParsedMemo::default();
        assert!(!apply_upsert(&mut parsed, "部署路径", "- /opt/app"));
        assert!(!apply_upsert(&mut parsed, "特殊命令", "- cmd"));
        // 替换第一段：顺序不变、不产生重复段。
        assert!(apply_upsert(&mut parsed, "部署路径", "- /srv/app\n- /etc/app.conf"));
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].title, "部署路径");
        assert_eq!(parsed.sections[0].body, "- /srv/app\n- /etc/app.conf");
        assert_eq!(parsed.sections[1].title, "特殊命令");
    }

    #[test]
    fn apply_remove_only_matching_section() {
        let mut parsed = ParsedMemo::default();
        apply_upsert(&mut parsed, "部署路径", "- /opt/app");
        apply_upsert(&mut parsed, "特殊命令", "- cmd");
        assert!(apply_remove(&mut parsed, "部署路径"));
        assert!(!apply_remove(&mut parsed, "不存在"));
        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.sections[0].title, "特殊命令");
    }

    #[test]
    fn title_validation_normalizes_and_rejects() {
        assert_eq!(validate_memo_title("  部署路径 ").unwrap(), "部署路径");
        // 容忍模型带上前缀。
        assert_eq!(validate_memo_title("## 部署路径").unwrap(), "部署路径");
        assert!(validate_memo_title("   ").is_err());
        assert!(validate_memo_title("##").is_err());
        assert!(validate_memo_title("a\nb").is_err());
        let long: String = "长".repeat(TITLE_MAX_CHARS + 1);
        assert!(validate_memo_title(&long).is_err());
        let max: String = "长".repeat(TITLE_MAX_CHARS);
        assert!(validate_memo_title(&max).is_ok());
    }

    #[test]
    fn check_limits_enforces_total_and_section_caps() {
        let mut parsed = ParsedMemo::default();
        // 单段超限：SECTION_MAX_CHARS + 1。
        let big_body = "长".repeat(SECTION_MAX_CHARS + 1);
        apply_upsert(&mut parsed, "大段", &big_body);
        let rendered = render_memo(&parsed);
        let error = check_limits(&rendered, &parsed).unwrap_err();
        assert!(error.contains("单段上限"), "{error}");

        // 全文超限：3 段各 3000 字符（单段合法，总量 9000+ > 8000）。
        let mut parsed = ParsedMemo::default();
        for index in 0..3 {
            apply_upsert(&mut parsed, &format!("段{index}"), &"长".repeat(3000));
        }
        let rendered = render_memo(&parsed);
        let error = check_limits(&rendered, &parsed).unwrap_err();
        assert!(error.contains(&format!("超过备忘录上限 {MEMO_MAX_CHARS}")), "{error}");
        // 错误消息带各段长度清单，供调用方决定精简哪段。
        assert!(error.contains("段0: 3000"), "{error}");
        assert!(error.contains("段2: 3000"), "{error}");

        // 合法内容通过。
        let mut parsed = ParsedMemo::default();
        apply_upsert(&mut parsed, "部署路径", "- /opt/app");
        let rendered = render_memo(&parsed);
        assert!(check_limits(&rendered, &parsed).is_ok());
    }

    #[test]
    fn atomic_write_read_delete_roundtrip() {
        let root = temp_root();
        let path = memo_path_in(&root, "srv-1").unwrap();
        assert_eq!(read_content_at(&path).unwrap(), "");

        let mut parsed = ParsedMemo::default();
        apply_upsert(&mut parsed, "部署路径", "- /opt/app");
        atomic_write_at(&path, &render_memo(&parsed)).unwrap();

        let content = read_content_at(&path).unwrap();
        assert!(content.starts_with(GENERATED_HEADER_PREFIX));
        assert_eq!(parse_sections(&content).sections[0].title, "部署路径");
        assert!(file_mtime_rfc3339(&path).is_some());
        // 临时文件不残留。
        assert!(!path.with_file_name(".srv-1.md.tmp").exists());

        delete_file_at(&path).unwrap();
        // 幂等：再删一次不报错。
        delete_file_at(&path).unwrap();
        assert_eq!(read_content_at(&path).unwrap(), "");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_memo_at_missing_file_is_empty_success() {
        let root = temp_root();
        let path = memo_path_in(&root, "no-such").unwrap();
        let payload = read_memo_at(&path).unwrap();
        assert_eq!(payload.content, "");
        assert_eq!(payload.updated_at, None);
        fs::remove_dir_all(&root).ok();
    }
}
