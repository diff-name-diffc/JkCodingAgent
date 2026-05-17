// ── Session metrics ───────────────────────────────────────────────────────────

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(serde::Serialize, Clone, Default)]
pub(crate) struct SessionMetrics {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) tool_calls: u64,
    pub(crate) duration_secs: f64,
}

/// 缓存：session_path → (file_modified_time, SessionMetrics)
static METRICS_CACHE: Lazy<Mutex<HashMap<String, (SystemTime, SessionMetrics)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(crate) fn parse_session_metrics_from_path(path: &std::path::Path) -> SessionMetrics {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            return SessionMetrics {
                input_tokens: 0,
                output_tokens: 0,
                tool_calls: 0,
                duration_secs: 0.0,
            }
        }
    };

    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut tool_calls: u64 = 0;
    let mut first_ts: Option<f64> = None;
    let mut last_ts: Option<f64> = None;

    for line in content.lines() {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if let Some(ts_str) = val.get("timestamp").and_then(|v| v.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                let ts = dt.timestamp() as f64 + dt.timestamp_subsec_millis() as f64 / 1000.0;
                if first_ts.is_none() {
                    first_ts = Some(ts);
                }
                last_ts = Some(ts);
            }
        }

        let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "assistant" {
            continue;
        }

        if let Some(message) = val.get("message") {
            if let Some(usage) = message.get("usage") {
                input_tokens += usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_tokens += usage
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }
            if let Some(content_arr) = message.get("content").and_then(|v| v.as_array()) {
                for item in content_arr {
                    if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        tool_calls += 1;
                    }
                }
            }
        }
    }

    let duration_secs = match (first_ts, last_ts) {
        (Some(first), Some(last)) => (last - first).max(0.0),
        _ => 0.0,
    };

    SessionMetrics {
        input_tokens,
        output_tokens,
        tool_calls,
        duration_secs,
    }
}

/// 带缓存的 session 指标解析
/// 通过文件修改时间判断缓存是否有效，避免重复解析未变更的文件
pub(crate) fn parse_session_metrics_cached(path: &std::path::Path) -> SessionMetrics {
    let path_str = path.to_string_lossy().to_string();

    // 获取文件修改时间
    let modified = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return SessionMetrics::default(),
    };

    // 检查缓存
    {
        let cache = METRICS_CACHE.lock();
        if let Some((cached_time, cached_metrics)) = cache.get(&path_str) {
            if *cached_time == modified {
                return cached_metrics.clone();
            }
        }
    }

    // 缓存未命中，完整解析
    let metrics = parse_session_metrics_from_path(path);

    // 更新缓存
    {
        let mut cache = METRICS_CACHE.lock();
        cache.insert(path_str, (modified, metrics.clone()));
    }

    metrics
}

#[tauri::command]
pub async fn read_session_metrics(session_path: String) -> Result<SessionMetrics, String> {
    let path = std::path::Path::new(&session_path);
    if !path.exists() {
        return Err(format!("Session file not found: {}", session_path));
    }
    Ok(parse_session_metrics_from_path(path))
}

// ── Weekly analytics ──────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct DayStats {
    pub date: String,
    pub task_count: u32,
    pub done_count: u32,
    pub token_count: u64,
}

#[derive(serde::Serialize)]
pub struct ProjectAnalytics {
    pub project_id: String,
    pub project_name: String,
    pub task_count: u32,
    pub done_count: u32,
    pub token_count: u64,
    pub tool_calls: u64,
}

#[derive(serde::Serialize)]
pub struct WeeklyAnalytics {
    pub daily: Vec<DayStats>,
    pub total_tasks: u32,
    pub done_tasks: u32,
    pub failed_tasks: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tool_calls: u64,
    pub total_duration_secs: f64,
    pub claude_tasks: u32,
    pub codex_tasks: u32,
    pub projects: Vec<ProjectAnalytics>,
}

#[tauri::command]
pub async fn get_weekly_analytics() -> Result<WeeklyAnalytics, String> {
    use chrono::{Duration, Local};

    let today = Local::now().date_naive();
    // Build a list of the last 7 dates (oldest first)
    let dates: Vec<String> = (0..7i64)
        .rev()
        .map(|i| (today - Duration::days(i)).format("%Y-%m-%d").to_string())
        .collect();

    let cutoff_ms = (Local::now() - Duration::days(7)).timestamp_millis();

    // Load all projects
    let projects = load_projects()?;

    let mut daily_map: HashMap<String, DayStats> = dates
        .iter()
        .map(|d| {
            (
                d.clone(),
                DayStats {
                    date: d.clone(),
                    task_count: 0,
                    done_count: 0,
                    token_count: 0,
                },
            )
        })
        .collect();

    let mut project_map: HashMap<String, ProjectAnalytics> = HashMap::new();
    let mut total_tasks: u32 = 0;
    let mut done_tasks: u32 = 0;
    let mut failed_tasks: u32 = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_tool_calls: u64 = 0;
    let mut total_duration_secs: f64 = 0.0;
    let mut claude_tasks: u32 = 0;
    let mut codex_tasks: u32 = 0;

    for project in &projects {
        let tasks = load_project_tasks(project.id.clone())?;

        for task in &tasks {
            if task.created_at < cutoff_ms {
                continue;
            }

            // Determine date bucket
            let task_date = chrono::DateTime::from_timestamp_millis(task.created_at)
                .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d").to_string())
                .unwrap_or_default();

            total_tasks += 1;
            if task.status == "done" {
                done_tasks += 1;
            }
            if task.status == "failed" {
                failed_tasks += 1;
            }
            if task.agent == "claude" {
                claude_tasks += 1;
            } else {
                codex_tasks += 1;
            }

            // Read session metrics if available
            let session_path = task
                .claude_session_path
                .as_deref()
                .or(task.codex_session_path.as_deref());

            let (tok_in, tok_out, tc, dur) = if let Some(sp) = session_path {
                let p = std::path::Path::new(sp);
                if p.exists() {
                    let m = parse_session_metrics_cached(p);
                    (
                        m.input_tokens,
                        m.output_tokens,
                        m.tool_calls,
                        m.duration_secs,
                    )
                } else {
                    (0, 0, 0, 0.0)
                }
            } else {
                (0, 0, 0, 0.0)
            };

            total_input_tokens += tok_in;
            total_output_tokens += tok_out;
            total_tool_calls += tc;
            total_duration_secs += dur;

            let token_count = tok_in + tok_out;

            // Update daily bucket
            if let Some(day) = daily_map.get_mut(&task_date) {
                day.task_count += 1;
                if task.status == "done" {
                    day.done_count += 1;
                }
                day.token_count += token_count;
            }

            // Update project bucket
            let proj_entry =
                project_map
                    .entry(project.id.clone())
                    .or_insert_with(|| ProjectAnalytics {
                        project_id: project.id.clone(),
                        project_name: project.name.clone(),
                        task_count: 0,
                        done_count: 0,
                        token_count: 0,
                        tool_calls: 0,
                    });
            proj_entry.task_count += 1;
            if task.status == "done" {
                proj_entry.done_count += 1;
            }
            proj_entry.token_count += token_count;
            proj_entry.tool_calls += tc;
        }
    }

    let mut daily: Vec<DayStats> = dates.iter().filter_map(|d| daily_map.remove(d)).collect();
    daily.sort_by(|a, b| a.date.cmp(&b.date));

    let mut project_list: Vec<ProjectAnalytics> = project_map.into_values().collect();
    project_list.sort_by(|a, b| b.task_count.cmp(&a.task_count));

    Ok(WeeklyAnalytics {
        daily,
        total_tasks,
        done_tasks,
        failed_tasks,
        total_input_tokens,
        total_output_tokens,
        total_tool_calls,
        total_duration_secs,
        claude_tasks,
        codex_tasks,
        projects: project_list,
    })
}
use super::storage::{load_project_tasks, load_projects};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    static SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let id = SEED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nezha_test_analytics_{}_{}_{}",
            label,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn write_jsonl(path: &std::path::Path, lines: &[&str]) {
        let mut f = std::fs::File::create(path).expect("create jsonl");
        for line in lines {
            writeln!(f, "{line}").expect("write line");
        }
    }

    // ── SessionMetrics default ────────────────────────────────────────────────

    #[test]
    fn session_metrics_default_is_zero() {
        let m = SessionMetrics::default();
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
        assert_eq!(m.duration_secs, 0.0);
    }

    // ── parse_session_metrics_from_path ───────────────────────────────────────

    #[test]
    fn parse_metrics_from_nonexistent_file_returns_zeros() {
        let m = parse_session_metrics_from_path(std::path::Path::new(
            "/nonexistent_file_xyz.jsonl",
        ));
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
    }

    #[test]
    fn parse_metrics_from_empty_file_returns_zeros() {
        let dir = unique_dir("empty_file");
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").expect("write empty");

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
        assert_eq!(m.duration_secs, 0.0);
    }

    #[test]
    fn parse_metrics_counts_input_and_output_tokens() {
        let dir = unique_dir("tokens");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":200},"content":[]}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:01:00Z","message":{"usage":{"input_tokens":50,"output_tokens":75},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 150);
        assert_eq!(m.output_tokens, 275);
    }

    #[test]
    fn parse_metrics_counts_tool_calls() {
        let dir = unique_dir("tool_calls");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{},"content":[{"type":"tool_use","name":"read_file"},{"type":"tool_use","name":"write_file"},{"type":"text","text":"hello"}]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.tool_calls, 2);
    }

    #[test]
    fn parse_metrics_ignores_non_assistant_messages() {
        let dir = unique_dir("non_assistant");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"human","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":999}}}"#,
                r#"{"type":"system","timestamp":"2025-01-01T00:00:00Z"}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
    }

    #[test]
    fn parse_metrics_calculates_duration() {
        let dir = unique_dir("duration");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"usage":{},"content":[]}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:05.500Z","message":{"usage":{},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert!((m.duration_secs - 5.5).abs() < 0.1);
    }

    #[test]
    fn parse_metrics_single_message_has_zero_duration() {
        let dir = unique_dir("single_msg");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"usage":{},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.duration_secs, 0.0);
    }

    #[test]
    fn parse_metrics_skips_malformed_json_lines() {
        let dir = unique_dir("malformed");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                "not valid json",
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":10},"content":[]}}"#,
                "",
                r#"{"broken"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 10);
    }

    #[test]
    fn parse_metrics_handles_missing_usage_gracefully() {
        let dir = unique_dir("no_usage");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
    }

    #[test]
    fn parse_metrics_handles_missing_content_gracefully() {
        let dir = unique_dir("no_content");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":5}}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 5);
        assert_eq!(m.tool_calls, 0);
    }

    #[test]
    fn parse_metrics_handles_non_numeric_token_values() {
        let dir = unique_dir("non_numeric");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":"not_a_number"},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        // Should default to 0 for non-numeric values
        assert_eq!(m.input_tokens, 0);
    }

    // ── parse_session_metrics_cached ──────────────────────────────────────────

    #[test]
    fn cached_metrics_returns_same_result_as_uncached() {
        let dir = unique_dir("cached_same");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":42,"output_tokens":7},"content":[]}}"#,
            ],
        );

        let uncached = parse_session_metrics_from_path(&path);
        let cached = parse_session_metrics_cached(&path);

        assert_eq!(uncached.input_tokens, cached.input_tokens);
        assert_eq!(uncached.output_tokens, cached.output_tokens);
    }

    #[test]
    fn cached_metrics_returns_default_for_nonexistent() {
        let m = parse_session_metrics_cached(std::path::Path::new(
            "/nonexistent_cached_xyz.jsonl",
        ));
        assert_eq!(m.input_tokens, 0);
    }

    #[test]
    fn cached_metrics_refreshes_after_file_modification() {
        let dir = unique_dir("cached_refresh");
        let path = dir.join("session.jsonl");

        // Write initial content
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":10},"content":[]}}"#,
            ],
        );
        let m1 = parse_session_metrics_cached(&path);
        assert_eq!(m1.input_tokens, 10);

        // Small sleep to ensure mtime changes
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Overwrite with different content
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":99},"content":[]}}"#,
            ],
        );
        let m2 = parse_session_metrics_cached(&path);
        assert_eq!(m2.input_tokens, 99);
    }

    // ── DayStats / ProjectAnalytics / WeeklyAnalytics serialization ───────────

    #[test]
    fn day_stats_serializes() {
        let ds = DayStats {
            date: "2025-01-01".to_string(),
            task_count: 5,
            done_count: 3,
            token_count: 1000,
        };
        let json = serde_json::to_string(&ds).expect("serialize DayStats");
        assert!(json.contains("\"date\":\"2025-01-01\""));
        assert!(json.contains("\"task_count\":5"));
        assert!(json.contains("\"token_count\":1000"));
    }

    #[test]
    fn project_analytics_serializes() {
        let pa = ProjectAnalytics {
            project_id: "p1".to_string(),
            project_name: "MyProject".to_string(),
            task_count: 10,
            done_count: 8,
            token_count: 5000,
            tool_calls: 200,
        };
        let json = serde_json::to_string(&pa).expect("serialize");
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"tool_calls\":200"));
    }

    #[test]
    fn weekly_analytics_serializes_with_all_fields() {
        let wa = WeeklyAnalytics {
            daily: vec![DayStats {
                date: "2025-01-01".to_string(),
                task_count: 1,
                done_count: 1,
                token_count: 100,
            }],
            total_tasks: 1,
            done_tasks: 1,
            failed_tasks: 0,
            total_input_tokens: 60,
            total_output_tokens: 40,
            total_tool_calls: 5,
            total_duration_secs: 120.5,
            claude_tasks: 1,
            codex_tasks: 0,
            projects: vec![],
        };
        let json = serde_json::to_string(&wa).expect("serialize");
        assert!(json.contains("\"total_tasks\":1"));
        assert!(json.contains("\"total_duration_secs\":120.5"));
        assert!(json.contains("\"claude_tasks\":1"));
        assert!(json.contains("\"codex_tasks\":0"));
    }

    // ── read_session_metrics (Tauri command) ──────────────────────────────────

    #[tokio::test]
    async fn read_session_metrics_file_not_found() {
        let result = read_session_metrics("/nonexistent_session_xyz.jsonl".to_string()).await;
        assert!(result.is_err());
        let err_msg = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(
            err_msg.contains("Session file not found"),
            "error should mention file not found, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn read_session_metrics_parses_valid_file() {
        let dir = unique_dir("read_metrics");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":200},"content":[{"type":"tool_use","name":"read"}]}}"#,
            ],
        );

        let result = read_session_metrics(path.to_string_lossy().to_string()).await;
        assert!(result.is_ok());
        let m = result.unwrap();
        assert_eq!(m.input_tokens, 100);
        assert_eq!(m.output_tokens, 200);
        assert_eq!(m.tool_calls, 1);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Additional integration tests for analytics commands
    // ══════════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn read_session_metrics_with_multiple_tool_calls_and_messages() {
        let dir = unique_dir("multi_msg");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"usage":{"input_tokens":500,"output_tokens":100},"content":[{"type":"tool_use","name":"read_file"},{"type":"tool_use","name":"write_file"}]}}"#,
                r#"{"type":"human","timestamp":"2025-01-01T00:00:10.000Z","message":{"content":"now fix it"}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:15.000Z","message":{"usage":{"input_tokens":800,"output_tokens":300},"content":[{"type":"tool_use","name":"exec"},{"type":"text","text":"done"}]}}"#,
            ],
        );

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 1300); // 500 + 800
        assert_eq!(m.output_tokens, 400); // 100 + 300
        assert_eq!(m.tool_calls, 3); // 2 + 1
        assert!((m.duration_secs - 15.0).abs() < 0.1, "duration should be ~15s");
    }

    #[tokio::test]
    async fn read_session_metrics_with_empty_jsonl() {
        let dir = unique_dir("empty_jsonl");
        let path = dir.join("empty.jsonl");
        write_jsonl(&path, &["", "", ""]);

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
        assert_eq!(m.duration_secs, 0.0);
    }

    #[tokio::test]
    async fn read_session_metrics_with_only_human_messages() {
        let dir = unique_dir("human_only");
        let path = dir.join("human.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"human","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":999}}}"#,
                r#"{"type":"human","timestamp":"2025-01-01T00:01:00Z","message":{"usage":{"input_tokens":888}}}"#,
            ],
        );

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
    }

    #[tokio::test]
    async fn read_session_metrics_with_timestamp_but_no_usage() {
        let dir = unique_dir("no_usage_ts");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"thinking..."}]}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:30.500Z","message":{"content":[{"type":"text","text":"done"}]}}"#,
            ],
        );

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert_eq!(m.tool_calls, 0);
        assert!(
            (m.duration_secs - 30.5).abs() < 0.1,
            "duration should be computed from timestamps even without usage"
        );
    }

    #[tokio::test]
    async fn read_session_metrics_malformed_timestamp_ignored() {
        let dir = unique_dir("bad_ts");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"not-a-date","message":{"usage":{"input_tokens":10},"content":[]}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"usage":{"input_tokens":20},"content":[]}}"#,
            ],
        );

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 30);
    }

    #[tokio::test]
    async fn read_session_metrics_mixed_valid_and_invalid_lines() {
        let dir = unique_dir("mixed_lines");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                "this is not json at all",
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":42,"output_tokens":7},"content":[]}}"#,
                "",
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:05Z","message":{"usage":{"input_tokens":8},"content":[{"type":"tool_use","name":"grep"}]}}"#,
                r#"{"broken json"#,
            ],
        );

        let m = read_session_metrics(path.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(m.input_tokens, 50); // 42 + 8
        assert_eq!(m.output_tokens, 7);
        assert_eq!(m.tool_calls, 1);
    }

    // ── parse_session_metrics_from_path edge cases ────────────────────────────

    #[test]
    fn parse_metrics_large_token_values() {
        let dir = unique_dir("large_tokens");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":4294967295,"output_tokens":4294967295},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 4294967295);
        assert_eq!(m.output_tokens, 4294967295);
    }

    #[test]
    fn parse_metrics_zero_token_values() {
        let dir = unique_dir("zero_tokens");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"usage":{"input_tokens":0,"output_tokens":0},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
    }

    #[test]
    fn parse_metrics_duration_never_negative() {
        let dir = unique_dir("neg_dur");
        let path = dir.join("session.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2025-01-01T00:01:00.000Z","message":{"usage":{},"content":[]}}"#,
                r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00.000Z","message":{"usage":{},"content":[]}}"#,
            ],
        );

        let m = parse_session_metrics_from_path(&path);
        assert!(m.duration_secs >= 0.0);
    }
}
