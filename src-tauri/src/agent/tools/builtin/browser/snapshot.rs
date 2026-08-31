//! browser_read_text 的快照缓存与行号分页渲染。
//!
//! 每个 workspace 缓存最近一次全量快照：带 offset/limit 的分页读取直接命中缓存，
//! 不重复请求 CDP，既保证行号与模型刚看到的快照一致，也避免刷新 sidecar 的
//! ref 映射导致此前下发的 ref 全部失效。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde_json::Value;

use super::format_browser_result;

/// read_text 不传 limit 时读取的行数上限（与 read_file 的默认 limit 对齐）。
pub(super) const READ_TEXT_DEFAULT_LINE_LIMIT: usize = 2_000;

/// sidecar read_text 结果的解析产物：结构化元信息 + 快照树行。
#[derive(Clone)]
pub(super) struct SnapshotContent {
    pub url: String,
    pub node_count: u64,
    pub emitted: u64,
    pub ref_count: u64,
    pub truncated: bool,
    pub lines: Vec<String>,
}

struct CachedSnapshot {
    snapshot_id: u64,
    content: SnapshotContent,
}

/// 全局快照序号：让模型能察觉两次分页读取之间快照是否已被刷新。
static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn snapshot_cache() -> &'static Mutex<HashMap<String, CachedSnapshot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedSnapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 丢弃某工作区的缓存快照（导航/关闭浏览器后 sidecar 的 ref 映射已失效）。
pub(super) fn invalidate_cached_snapshot(workspace_id: &str) {
    snapshot_cache().lock().remove(workspace_id);
}

/// 读取缓存快照的分页切片；无缓存（sidecar 重启 / 冷启动）返回 None。
pub(super) fn render_cached_page(
    workspace_id: &str,
    offset: usize,
    limit: usize,
) -> Option<String> {
    let cache = snapshot_cache().lock();
    let snapshot = cache.get(workspace_id)?;
    Some(render_snapshot_page(
        &snapshot.content,
        None,
        Some(snapshot.snapshot_id),
        offset,
        limit,
    ))
}

fn store_snapshot(workspace_id: &str, content: SnapshotContent) -> u64 {
    let snapshot_id = SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    snapshot_cache().lock().insert(
        workspace_id.to_string(),
        CachedSnapshot {
            snapshot_id,
            content,
        },
    );
    snapshot_id
}

/// 从 sidecar read_text 返回值提取快照内容。sidecar 的 text 自带
/// `# Accessibility Tree Snapshot\nurl: …\nnode_count: …\ntruncated: …\n\n` 头部，
/// 元信息改由结构化字段渲染，只有头部之后的树行才参与行号分页——否则整棵树
/// 会挤在 pretty JSON 的单个字符串行里，行级截断定位完全失效。
pub(super) fn extract_snapshot(value: &Value) -> Option<SnapshotContent> {
    let text = value.get("text")?.as_str()?;
    let tree = text
        .split_once("\n\n")
        .map(|(_, tree)| tree)
        .unwrap_or(text);
    Some(SnapshotContent {
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        node_count: value.get("nodeCount").and_then(Value::as_u64).unwrap_or(0),
        emitted: value
            .get("emittedNodeCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        ref_count: value.get("refCount").and_then(Value::as_u64).unwrap_or(0),
        truncated: value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        lines: tree.lines().map(str::to_string).collect(),
    })
}

/// 渲染快照的一页：元信息头 + `行号|内容` 树行切片，行号格式与 read_file 一致，
/// 通用管线的字符截断定位器可直接命中行号前缀。offset/limit 语义与 read_file
/// 对齐：1 起始、包含边界、越界时返回可用内容。
pub(super) fn render_snapshot_page(
    content: &SnapshotContent,
    ref_arg: Option<&str>,
    snapshot_id: Option<u64>,
    offset: usize,
    limit: usize,
) -> String {
    let total = content.lines.len();
    let start = offset.max(1);
    let limit = limit.max(1);

    let mut header = match ref_arg {
        Some(ref_id) => format!("# Accessibility Tree Snapshot（ref={ref_id} 局部树）"),
        None => "# Accessibility Tree Snapshot".to_string(),
    };
    header.push_str(&format!("\nurl: {}", content.url));
    if ref_arg.is_some() {
        header.push_str(&format!(
            "\nnodes: {} emitted / {} refs",
            content.emitted, content.ref_count
        ));
    } else {
        header.push_str(&format!(
            "\nnodes: {} total / {} emitted / {} refs",
            content.node_count, content.emitted, content.ref_count
        ));
    }
    header.push_str(&format!("\nsnapshot_truncated: {}", content.truncated));
    if let Some(id) = snapshot_id {
        header.push_str(&format!("\nsnapshot: #{id}"));
    }

    if start > total {
        return format!("{header}\n\n（起始行 {start} 超出快照总行数 {total}，无内容返回）");
    }

    let end = (start + limit - 1).min(total);
    let mut parts = vec![
        header,
        format!("lines: {start}-{end} / {total}"),
        String::new(),
    ];
    for (index, line) in content.lines[start - 1..end].iter().enumerate() {
        parts.push(format!("{}|{}", start + index, line));
    }
    // 只陈述剩余行数，不引导下一步动作——是否继续读取由 Agent 决定
    // （头部的 lines: start-end / total 已足够推算后续行号）。
    if end < total {
        parts.push(String::new());
        parts.push(format!("（还有 {} 行未返回）", total - end));
    }
    parts.join("\n")
}

/// 全量/局部读取 sidecar 快照后：登记缓存（仅全量）并渲染行号分页结果。
/// sidecar 返回结构不符合预期时退回整包 JSON 展示。
pub(super) fn format_snapshot_response(
    value: Value,
    workspace_id: &str,
    ref_arg: Option<&str>,
    offset: usize,
    limit: usize,
) -> String {
    let Some(content) = extract_snapshot(&value) else {
        return format_browser_result(value);
    };
    let snapshot_id = (ref_arg.is_none()).then(|| store_snapshot(workspace_id, content.clone()));
    render_snapshot_page(&content, ref_arg, snapshot_id, offset, limit)
}

#[cfg(test)]
mod tests {
    use super::{extract_snapshot, render_snapshot_page};
    use serde_json::json;

    fn sample_snapshot_value() -> serde_json::Value {
        json!({
            "text": "# Accessibility Tree Snapshot\nurl: https://example.com/\nnode_count: 5\ntruncated: false\n\n- button [ref=r1] \"登录\"\n  - textbox [ref=r2] \"用户名\" value=\"jk\"\n- link [ref=r3] \"下一页\"",
            "nodeCount": 5,
            "emittedNodeCount": 3,
            "refCount": 3,
            "truncated": false,
            "url": "https://example.com/"
        })
    }

    #[test]
    fn extract_snapshot_separates_sidecar_header_from_tree_lines() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        assert_eq!(content.url, "https://example.com/");
        assert_eq!(content.node_count, 5);
        assert_eq!(content.emitted, 3);
        assert_eq!(content.ref_count, 3);
        assert!(!content.truncated);
        assert_eq!(
            content.lines,
            vec![
                "- button [ref=r1] \"登录\"",
                "  - textbox [ref=r2] \"用户名\" value=\"jk\"",
                "- link [ref=r3] \"下一页\""
            ]
        );
    }

    #[test]
    fn render_snapshot_page_numbers_all_lines_by_default() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, Some(7), 1, 2_000);
        assert!(page.contains("# Accessibility Tree Snapshot\nurl: https://example.com/"));
        assert!(page.contains("nodes: 5 total / 3 emitted / 3 refs"));
        assert!(page.contains("snapshot: #7"));
        assert!(page.contains("lines: 1-3 / 3"));
        assert!(page.contains("1|- button [ref=r1] \"登录\""));
        assert!(page.contains("3|- link [ref=r3] \"下一页\""));
        // 已读到末行，不再输出翻页提示
        assert!(!page.contains("继续读取"));
    }

    #[test]
    fn render_snapshot_page_slices_inclusive_range_and_states_remaining_neutrally() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, None, 2, 1);
        assert!(page.contains("lines: 2-2 / 3"));
        assert!(page.contains("2|  - textbox [ref=r2] \"用户名\" value=\"jk\""));
        assert!(!page.contains("1|- button"));
        // 只陈述剩余行数，不出现指令式引导
        assert!(page.contains("（还有 1 行未返回）"));
        assert!(!page.contains("继续读取"));
        assert!(!page.contains("offset=3"));
    }

    #[test]
    fn render_snapshot_page_reports_offset_beyond_total() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, None, 10, 50);
        assert!(page.contains("起始行 10 超出快照总行数 3，无内容返回"));
        assert!(!page.contains("请省略"));
        assert!(!page.contains("|-"));
    }

    #[test]
    fn render_snapshot_page_marks_partial_tree_scope() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, Some("r12"), None, 1, 2_000);
        assert!(page.contains("# Accessibility Tree Snapshot（ref=r12 局部树）"));
        assert!(page.contains("nodes: 3 emitted / 3 refs"));
    }
}
