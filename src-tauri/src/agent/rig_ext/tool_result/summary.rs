//! 双标签工具结果摘要协议（DISPLAY_SUMMARY / CONTEXT_PAYLOAD）。
//! 文本与解析规则复制自 `summary/tool_summary.rs`（该模块函数为私有不可
//! 复用），T4.1 迁移 summary 体系时归一为单一实现。

use super::TOOL_RESULT_INLINE_MAX_CHARS;

/// 上下文回写负载的篇幅预算。`bound_inline_tool_result` 会把负载硬性截断到
/// `TOOL_RESULT_INLINE_MAX_CHARS`，预算略低于该上限，引导摘要模型自行收口，
/// 避免硬截断切在内容段中间。
/// 送入摘要模型的原文上限（对齐旧 `summary::DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS`）。
pub(super) const DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS: usize = 24_000;

const CONTEXT_PAYLOAD_BUDGET_CHARS: usize = TOOL_RESULT_INLINE_MAX_CHARS - 200;
// 保证预算恒为正：上限调低时在编译期报错，而非静默溢出或产生无意义预算。
const _: () = assert!(TOOL_RESULT_INLINE_MAX_CHARS > 200);

/// 意图压缩分支的回写负载预算：调用方已声明「只要与意图相关的重点」，
/// 过长的输出本身就是冗余。预算远小于保守分支，引导模型输出精炼要点；
/// 确有必要的关键原文摘录（报错行、配置值）优先保留。
const INTENT_CONTEXT_PAYLOAD_BUDGET_CHARS: usize = 2_000;
const _: () = assert!(INTENT_CONTEXT_PAYLOAD_BUDGET_CHARS < TOOL_RESULT_INLINE_MAX_CHARS);

const LOCATOR_RULE: &str = "定位必须精确、可复查：按「内容摘要：…」+「内容定位：path:start-end」组织成一段或多段，不连续的相关内容拆成多段，一段可列出多个原文定位。路径和行号只能来自原始输出，严禁猜测或伪造；原始输出没有路径或行号时，改用原文中可复查的命令、标题或键名定位，并明确注明「原始输出未提供行号」。";

fn dual_summary_protocol(context_payload_guidance: &str, payload_budget_chars: usize) -> String {
    format!(
        "输出协议：只能使用以下两个标签，不得输出任何其他文字、标题或 Markdown 代码块。\n\
         <DISPLAY_SUMMARY>\n写给前端用户：用 1-3 句话概括本次工具调用的关键发现。\n</DISPLAY_SUMMARY>\n\
         <CONTEXT_PAYLOAD>\n{context_payload_guidance}总量控制在 {payload_budget_chars} 字符以内，超预算时优先保留定位信息与最关键的原文摘录。\n</CONTEXT_PAYLOAD>"
    )
}

/// 摘要系统提示。两个分支：带「提取意图」= 意图聚焦、保真提取；
/// 无意图 = 保守压缩（用户问题作上下文）。
pub(super) fn build_summary_system_prompt(tool_name: &str, has_intent: bool) -> String {
    let focus = tool_summary_focus(tool_name);
    if has_intent {
        format!(
            "你是调度 Agent 的工具结果提取器。调用方模型带着明确的「提取意图」执行了工具，你的任务是从工具原始输出中抽取与该意图直接相关的内容，供调用方决定下一步动作。\n\
             规则：\n\
             - 意图优先：只提取与「提取意图」直接相关的内容，与意图无关的一律丢弃。严禁对全文做泛泛的主题概括，严禁用无关信息填充输出；原文中与意图相关的内容很少时，如实说明，不要用无关内容凑字数。\n\
             - 重点精炼：输出是直接回答意图的要点，不是原文缩编——先给出结论性答案，再附支撑它的最关键摘录；不要罗列背景信息，不要大段复述原文，宁精勿滥。\n\
             - 相关内容保真：与意图高度相关的代码、配置、命令结果、错误文本必须尽量原文摘录，不得改写含义，不得压缩到丢失细节；只有确认无关的内容才允许省略。{focus}\n\
             - {LOCATOR_RULE}\n\
             {}",
            dual_summary_protocol(
                "写给调用方模型：直接回答提取意图——一段或多段「内容摘要 + 内容定位」，只含与意图直接相关的关键事实和最必要的原文摘录（路径、行号、符号名、配置键、错误文本、数量）。",
                INTENT_CONTEXT_PAYLOAD_BUDGET_CHARS,
            )
        )
    } else {
        format!(
            "你是调度 Agent 的工具结果压缩器。工具原始输出过长，需要压缩成两份内容：一份回写给调用方模型继续完成任务，一份展示给前端用户。\n\
             规则：\n\
             - 只保留原文里明确出现的事实，不要猜测；不要过度归纳，宁可稍长，也不要丢掉影响后续判断的细节。\n\
             - 如果内容主要是代码、配置、逐行检索结果、文件清单或其他精确检索输出，只能做最轻量压缩，严禁改写代码含义、删除关键行号、文件名或配置键；{focus}\n\
             - 如果内容是命令输出，优先保留命令结果、失败原因、关键日志、测试失败项和退出状态。\n\
             - {LOCATOR_RULE}\n\
             {}",
            dual_summary_protocol(
                "写给调用方模型：高信息密度，按「内容摘要 + 内容定位」输出一段或多段，尽量保留原始顺序、关键实体名、符号名、配置键、错误文本、数量和退出状态。",
                CONTEXT_PAYLOAD_BUDGET_CHARS,
            )
        )
    }
}

pub(super) fn build_summary_user_message(
    tool_name: &str,
    raw_output: &str,
    user_question: Option<&str>,
    compress_intent: Option<&str>,
) -> String {
    let mut content = String::new();
    if let Some(intent) = compress_intent {
        content.push_str(&format!("<提取意图>\n{intent}\n</提取意图>\n"));
    }
    if let Some(question) = user_question.filter(|q| !q.trim().is_empty()) {
        content.push_str(&format!("<用户原始问题>\n{question}\n</用户原始问题>\n"));
    }
    content.push_str(&format!(
        "工具名：{tool_name}\n工具原始输出如下：\n{}",
        truncate_for_summary(normalize_tool_output(raw_output).trim())
    ));
    content
}

fn truncate_for_summary(raw_output: &str) -> String {
    let max_chars = DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS;
    let char_count = raw_output.chars().count();
    if char_count <= max_chars {
        return raw_output.to_string();
    }
    let truncated: String = raw_output.chars().take(max_chars).collect();
    format!("{truncated}\n\n[原始输出已截断至 {max_chars} 字符，原文共 {char_count} 字符]")
}

fn tool_summary_focus(tool_name: &str) -> &str {
    match tool_name {
        "read_file" => "保留关键文件路径、符号名、行号范围、配置键和能支持判断的核心实现细节",
        "list_dir" => {
            "保留最多两层的目录关系、关键文件名及其总行数，便于后续用 read_file path:start-end 精确加载"
        }
        "glob" => "保留目录层级、关键文件名、数量和显著的结构特征",
        "grep" => "保留匹配文件路径、行号、命中片段、上下文和能支撑后续 read_file 的关键关键词",
        _ => "保留后续判断最依赖的事实、路径、标识符和数量信息",
    }
}

fn normalize_tool_output(raw_output: &str) -> String {
    raw_output
        .split('\n')
        .map(normalize_tool_output_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn normalize_tool_output_line(line: &str) -> String {
    if line.trim().is_empty() {
        return String::new();
    }

    let indent_len = line
        .char_indices()
        .find(|(_, ch)| *ch != ' ' && *ch != '\t')
        .map(|(index, _)| index)
        .unwrap_or(line.len());
    let (indent, rest) = line.split_at(indent_len);
    let mut normalized = String::with_capacity(line.len());
    normalized.push_str(indent);

    // 折叠策略：行内任意长度的连续空格统一折叠为单个空格；
    // 行首缩进保留，行尾空格由下方 trim 去除。
    let mut space_run = 0usize;
    for ch in rest.chars() {
        if ch == ' ' {
            space_run += 1;
            if space_run == 1 {
                normalized.push(ch);
            }
            continue;
        }

        space_run = 0;
        normalized.push(ch);
    }

    normalized.trim_end_matches(' ').to_string()
}

/// 解析双标签输出：(context_payload, display_summary)。
pub(crate) fn parse_dual_tool_summary(output: &str) -> (String, String) {
    let context_payload = extract_tagged_block(output, "CONTEXT_PAYLOAD", "DISPLAY_SUMMARY");
    let display_summary = extract_tagged_block(output, "DISPLAY_SUMMARY", "CONTEXT_PAYLOAD");

    match (context_payload, display_summary) {
        (Some(context_payload), Some(display_summary)) => (context_payload, display_summary),
        // 摘要模型只产出一个区块时，另一侧复用同一内容，避免回退到带协议标签的原文。
        (Some(context_payload), None) => (context_payload.clone(), context_payload),
        (None, Some(display_summary)) => (display_summary.clone(), display_summary),
        (None, None) => {
            let fallback = strip_dual_summary_tags(output);
            (fallback.clone(), fallback)
        }
    }
}

/// 提取 `<TAG>…</TAG>` 区块。摘要模型并不总是闭合标签（实测会出现
/// `<DISPLAY_SUMMARY>` 后直接跟 `<CONTEXT_PAYLOAD>` 的输出），因此无闭合标签时
/// 回退到「另一个区块的起始标签」或文本末尾处截止。优先匹配闭合标签，
/// 防止正文里出现的字面 `<OTHER_TAG>` / `</TAG>` 残留把负载提前切断。
/// 起始标签缺失或内容为空时返回 None。
pub(crate) fn extract_tagged_block(output: &str, tag: &str, other_tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let start = output.find(&start_tag)?;
    let rest = &output[start + start_tag.len()..];
    let end = match rest.find(&format!("</{tag}>")) {
        Some(pos) => pos,
        None => {
            eprintln!("警告：摘要输出缺少 </{tag}> 闭合标签，按下一区块起始标签或文本末尾截止");
            rest.find(&format!("<{other_tag}>")).unwrap_or(rest.len())
        }
    };
    let trimmed = rest[..end].trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 解析彻底失败时剥掉双区块协议标签，避免 `<DISPLAY_SUMMARY>` /
/// `<CONTEXT_PAYLOAD>` 泄漏到聊天界面与模型上下文。
fn strip_dual_summary_tags(output: &str) -> String {
    output
        .replace("<DISPLAY_SUMMARY>", "")
        .replace("</DISPLAY_SUMMARY>", "")
        .replace("<CONTEXT_PAYLOAD>", "")
        .replace("</CONTEXT_PAYLOAD>", "")
        .trim()
        .to_string()
}
// ─── 工具结果摘要的确定性兜底（零 LLM 规则抽取） ──────────────────────────────
// 迁移自旧 `summary/tool_summary.rs`：摘要模型超时/失败时用规则抽取
// （头尾截断 + 各工具的关注点提示），保证压缩链路绝不因 LLM 不可用而中断。

pub(super) const STRUCTURED_SUMMARY_MAX_CHARS: usize = 8_000;
const STRUCTURED_SUMMARY_BODY_CHARS: usize = 6_000;

/// Pure rule-based extraction of key information from a tool result.
/// Zero LLM calls. Used as fallback when the summary model fails, reducing
/// the probability of the main model's content-moderation filter firing by
/// stripping large code blocks and repetitive log lines.
pub(crate) fn extract_structured_summary(tool_name: &str, raw_output: &str) -> String {
    let mut sections: Vec<String> = Vec::new();
    let char_count = raw_output.chars().count();
    let line_count = raw_output.lines().count();

    sections.push(format!(
        "[{tool_name}: {char_count} chars, {line_count} lines]"
    ));

    match tool_name {
        // 命令执行类输出的兜底策略：退出状态、错误/失败行、头尾行。
        // ssh_exec 的原始输出是含 stdout/stderr/exit_code 的 JSON 文本，
        // 同样的模式匹配仍然适用。
        "ssh_exec" => {
            let lines: Vec<&str> = raw_output.lines().collect();
            if let Some(exit) = lines.iter().rev().find(|l| {
                let t = l.trim().to_lowercase();
                // 只匹配明确的退出状态模式（exit code N / 退出状态：N 等）；
                // 不匹配 "$ 提示符"、含 "exit" 字样的普通日志或以 "error" 开头的行，
                // 匹配不到时不编造状态。
                t.contains("exit code") || t.contains("退出状态")
            }) {
                sections.push(format!("退出/状态: {exit}"));
            }
            let errors: Vec<&&str> = lines
                .iter()
                .filter(|l| {
                    let t = l.trim().to_lowercase();
                    t.contains("error")
                        || t.contains("fail")
                        || t.contains("panic")
                        || t.contains("failed")
                        || t.contains("失败")
                })
                .take(10)
                .collect();
            if !errors.is_empty() {
                sections.push(format!(
                    "错误/失败:\n{}",
                    errors.iter().map(|s| **s).collect::<Vec<_>>().join("\n")
                ));
            }
            let head: Vec<&str> = lines.iter().take(20).copied().collect();
            sections.push(head.join("\n"));
            if lines.len() > 30 {
                sections.push(format!("...(省略 {} 行)...", lines.len() - 30));
                let tail: Vec<&str> = lines.iter().rev().take(10).rev().copied().collect();
                sections.push(tail.join("\n"));
            }
        }
        "read_file" => {
            let symbols: Vec<&str> = raw_output
                .lines()
                .filter(|l| {
                    // Strip leading line-number prefix (e.g., "1|  " or "42 |  ").
                    // 仅剥离「行首数字+可选空格+|」形式，避免拆坏正文中含 `|`
                    // 的代码行（闭包、管道等）。
                    let stripped = l
                        .split_once('|')
                        .filter(|(prefix, _)| {
                            !prefix.trim().is_empty()
                                && prefix.trim().chars().all(|c| c.is_ascii_digit())
                        })
                        .map(|(_, rest)| rest)
                        .unwrap_or(l);
                    let t = stripped.trim();
                    t.starts_with("fn ")
                        || t.starts_with("pub fn")
                        || t.starts_with("pub(crate)")
                        || t.starts_with("async fn")
                        || t.starts_with("pub async fn")
                        || t.starts_with("class ")
                        || t.starts_with("def ")
                        || t.starts_with("import ")
                        || t.starts_with("from ")
                        || t.starts_with("const ")
                        || t.starts_with("interface ")
                        || t.starts_with("type ")
                        || t.starts_with("export ")
                        || t.starts_with("struct ")
                        || t.starts_with("enum ")
                })
                .take(50)
                .collect();
            if !symbols.is_empty() {
                sections.push(format!(
                    "符号定义:\n{}",
                    symbols.into_iter().collect::<Vec<_>>().join("\n")
                ));
            }
            sections.push(truncate_middle(
                raw_output,
                STRUCTURED_SUMMARY_BODY_CHARS,
                "代码体",
            ));
        }
        "grep" => {
            let matches: Vec<&str> = raw_output
                .lines()
                .filter(|l| l.contains(':') || l.contains("-->"))
                .take(100)
                .collect();
            sections.push(matches.into_iter().collect::<Vec<_>>().join("\n"));
            let total = raw_output.lines().count();
            if total > 100 {
                sections.push(format!("...(共 {total} 条匹配)..."));
            }
        }
        "glob" | "list_dir" => {
            let entries: Vec<&str> = raw_output.lines().take(200).collect();
            sections.push(entries.into_iter().collect::<Vec<_>>().join("\n"));
            let total = raw_output.lines().count();
            if total > 200 {
                sections.push(format!("...(共 {total} 项)..."));
            }
        }
        _ => {
            sections.push(truncate_middle(
                raw_output,
                STRUCTURED_SUMMARY_BODY_CHARS,
                "内容",
            ));
        }
    }

    let result = sections.join("\n");
    if result.chars().count() > STRUCTURED_SUMMARY_MAX_CHARS {
        truncate_middle(&result, STRUCTURED_SUMMARY_MAX_CHARS, "摘要")
    } else {
        result
    }
}

fn truncate_middle(text: &str, max_chars: usize, label: &str) -> String {
    let chars = text.chars().count();
    if chars <= max_chars {
        return text.to_string();
    }
    let half = max_chars / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(chars - half).collect();
    format!(
        "{head}\n[...省略 {} {label}字符...]\n{tail}",
        chars - max_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_summary_blocks_parse_with_missing_close_tag() {
        let output = "<DISPLAY_SUMMARY>\n摘要一句话\n<CONTEXT_PAYLOAD>\n负载内容";
        let (payload, display) = parse_dual_tool_summary(output);
        assert_eq!(payload, "负载内容");
        assert_eq!(display, "摘要一句话");
    }

    #[test]
    fn dual_summary_falls_back_to_stripped_text() {
        // 完全没有协议标签：剥标签兜底，返回原文。
        let (payload, display) = parse_dual_tool_summary("没有标签的摘要");
        assert_eq!(payload, "没有标签的摘要");
        assert_eq!(display, payload);
    }

    #[test]
    fn normalize_collapses_inner_spaces_but_keeps_indent() {
        assert_eq!(normalize_tool_output_line("  a   b  "), "  a b");
        assert_eq!(normalize_tool_output_line("   "), "");
    }
}
