//! Claude / Codex 会话文件（JSONL）解析。
//!
//! 旧 dispatch 链路（PTY 子进程 + 会话监视器）已整体下线；本文件只保留
//! `read_session_messages` 命令及其解析助手——按会话文件路径读取 claude/codex
//! 的历史会话内容，供前端会话查看器使用。

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

// ── Session messages (for conversation view) ──────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub(crate) struct SessionMessage {
    role: String,
    content: Vec<SessionContent>,
}

#[derive(serde::Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SessionContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    Thinking {
        thinking: String,
    },
}

#[tauri::command]
pub async fn read_session_messages(session_path: String) -> Result<Vec<SessionMessage>, String> {
    // Stream-read the JSONL line-by-line inside spawn_blocking so that neither
    // file I/O nor JSON parsing blocks the Tokio async runtime. Lines are capped
    // at MAX_LINES to bound memory for very large session files (hundreds of MB).
    tokio::task::spawn_blocking(move || -> Result<Vec<SessionMessage>, String> {
        use std::io::BufRead;

        const MAX_LINES: usize = 50000;
        let file = File::open(&session_path).map_err(|e| e.to_string())?;
        let reader = BufReader::with_capacity(256 * 1024, file);
        let mut owned_lines: Vec<String> = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            if owned_lines.len() >= MAX_LINES {
                break;
            }
            owned_lines.push(line);
        }

        let line_refs: Vec<&str> = owned_lines.iter().map(|s| s.as_str()).collect();
        if is_codex_format(&line_refs) {
            Ok(parse_codex_session(&line_refs))
        } else {
            Ok(parse_claude_session(&line_refs))
        }
    })
    .await
    .map_err(|e| format!("读取会话消息失败：{e}"))?
}

fn is_codex_format(lines: &[&str]) -> bool {
    for line in lines.iter().take(10) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            match val.get("type").and_then(|v| v.as_str()) {
                Some("session_meta") | Some("event_msg") => return true,
                _ => {}
            }
        }
    }
    false
}

fn parse_claude_session(lines: &[&str]) -> Vec<SessionMessage> {
    let mut messages = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for line in lines {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(message) = val.get("message") else {
            continue;
        };

        match msg_type {
            "user" => {
                let parts = claude_user_content(message.get("content"), &tool_names_by_id);
                if !parts.text_parts.is_empty() {
                    messages.push(SessionMessage {
                        role: "user".to_string(),
                        content: parts.text_parts,
                    });
                }
                append_assistant_session_parts(&mut messages, parts.tool_results);
            }
            "assistant" => {
                let parts = message
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|arr| claude_assistant_blocks(arr, &mut tool_names_by_id))
                    .unwrap_or_default();
                append_assistant_session_parts(&mut messages, parts);
            }
            _ => {}
        }
    }

    messages
}

struct ClaudeUserParts {
    text_parts: Vec<SessionContent>,
    tool_results: Vec<SessionContent>,
}

fn claude_user_content(
    content: Option<&serde_json::Value>,
    tool_names_by_id: &HashMap<String, String>,
) -> ClaudeUserParts {
    match content {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => ClaudeUserParts {
            text_parts: vec![SessionContent::Text { text: s.clone() }],
            tool_results: Vec::new(),
        },
        Some(serde_json::Value::Array(blocks)) => blocks.iter().fold(
            ClaudeUserParts {
                text_parts: Vec::new(),
                tool_results: Vec::new(),
            },
            |mut acc, block| {
                match block.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.trim().is_empty() {
                            acc.text_parts.push(SessionContent::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    Some("tool_result") => {
                        let id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if let Some(result) = json_value_to_display(block.get("content")) {
                            acc.tool_results.push(SessionContent::ToolResult {
                                id: id.clone(),
                                name: tool_names_by_id
                                    .get(&id)
                                    .cloned()
                                    .unwrap_or_else(|| "tool".to_string()),
                                result,
                            });
                        }
                    }
                    _ => {}
                }
                acc
            },
        ),
        _ => ClaudeUserParts {
            text_parts: Vec::new(),
            tool_results: Vec::new(),
        },
    }
}

fn claude_assistant_blocks(
    blocks: &[serde_json::Value],
    tool_names_by_id: &mut HashMap<String, String>,
) -> Vec<SessionContent> {
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        parts.push(SessionContent::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block
                    .get("input")
                    .and_then(|v| serde_json::to_string_pretty(v).ok())
                    .unwrap_or_default();
                if !id.is_empty() && !name.is_empty() {
                    tool_names_by_id.insert(id.clone(), name.clone());
                }
                parts.push(SessionContent::ToolUse { id, name, input });
            }
            Some("thinking") => {
                if let Some(thinking) = block.get("thinking").and_then(|v| v.as_str()) {
                    if !thinking.trim().is_empty() {
                        parts.push(SessionContent::Thinking {
                            thinking: thinking.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    parts
}

fn parse_codex_session(lines: &[&str]) -> Vec<SessionMessage> {
    let mut messages: Vec<SessionMessage> = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for line in lines {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = val.get("payload");

        match event_type {
            "event_msg" => {
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if payload_type == "user_message" {
                    let text = payload
                        .and_then(|p| p.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !text.trim().is_empty() {
                        messages.push(SessionMessage {
                            role: "user".to_string(),
                            content: vec![SessionContent::Text {
                                text: text.to_string(),
                            }],
                        });
                    }
                }
            }
            "response_item" => {
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                match payload_type {
                    "message" => {
                        let role = payload
                            .and_then(|p| p.get("role"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if role != "assistant" {
                            continue;
                        }
                        let parts: Vec<SessionContent> = payload
                            .and_then(|p| p.get("content"))
                            .and_then(|v| v.as_array())
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter_map(|b| {
                                        let t = b.get("type").and_then(|v| v.as_str())?;
                                        if matches!(t, "output_text" | "text") {
                                            let text = b.get("text").and_then(|v| v.as_str())?;
                                            if !text.trim().is_empty() {
                                                return Some(SessionContent::Text {
                                                    text: text.to_string(),
                                                });
                                            }
                                        }
                                        None
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !parts.is_empty() {
                            if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                                messages.last_mut().unwrap().content.extend(parts);
                            } else {
                                messages.push(SessionMessage {
                                    role: "assistant".to_string(),
                                    content: parts,
                                });
                            }
                        }
                    }
                    "function_call" => {
                        let call_id = payload
                            .and_then(|p| p.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = payload
                            .and_then(|p| p.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let raw = payload
                            .and_then(|p| p.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let input = serde_json::from_str::<serde_json::Value>(raw)
                            .ok()
                            .and_then(|v| serde_json::to_string_pretty(&v).ok())
                            .unwrap_or_else(|| raw.to_string());
                        if !call_id.is_empty() && !name.is_empty() {
                            tool_names_by_id.insert(call_id.clone(), name.clone());
                        }
                        let part = SessionContent::ToolUse {
                            id: call_id,
                            name,
                            input,
                        };
                        if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                            messages.last_mut().unwrap().content.push(part);
                        } else {
                            messages.push(SessionMessage {
                                role: "assistant".to_string(),
                                content: vec![part],
                            });
                        }
                    }
                    "function_call_output" => {
                        let call_id = payload
                            .and_then(|p| p.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let Some(result) =
                            json_value_to_display(payload.and_then(|p| p.get("output")))
                        else {
                            continue;
                        };
                        let part = SessionContent::ToolResult {
                            id: call_id.clone(),
                            name: tool_names_by_id
                                .get(&call_id)
                                .cloned()
                                .unwrap_or_else(|| "tool".to_string()),
                            result,
                        };
                        if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                            messages.last_mut().unwrap().content.push(part);
                        } else {
                            messages.push(SessionMessage {
                                role: "assistant".to_string(),
                                content: vec![part],
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    messages
}

fn append_assistant_session_parts(messages: &mut Vec<SessionMessage>, parts: Vec<SessionContent>) {
    if parts.is_empty() {
        return;
    }

    if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
        messages.last_mut().unwrap().content.extend(parts);
    } else {
        messages.push(SessionMessage {
            role: "assistant".to_string(),
            content: parts,
        });
    }
}

fn json_value_to_display(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(serde_json::Value::Array(items)) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                serde_json::to_string_pretty(items)
                    .ok()
                    .filter(|serialized| !serialized.trim().is_empty())
            } else {
                Some(text)
            }
        }
        Some(other) => serde_json::to_string_pretty(other)
            .ok()
            .filter(|serialized| !serialized.trim().is_empty()),
        None => None,
    }
}
