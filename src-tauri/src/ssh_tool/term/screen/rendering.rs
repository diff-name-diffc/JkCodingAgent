//! 屏幕/历史的文本表示：ANSI 输出只重建 avt 确认的字符与 SGR 样式。

use avt::{Color, Line, Pen, Vt};

use super::{NEW_LINES_MAX_CHARS, NEW_LINES_MAX_ROWS};

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct HistoryPage {
    /// terminal = 本地有限回滚；tmux = 远端当前活动 pane 的历史。
    pub(crate) source: &'static str,
    pub(crate) lines: Vec<String>,
    pub(crate) available_lines: usize,
    pub(crate) next_offset: Option<usize>,
    pub(crate) truncated: bool,
}

pub(super) fn history(vt: &Vt, offset: usize, limit: usize, ansi: bool) -> HistoryPage {
    let (_, rows) = vt.size();
    let available_lines = vt.lines().count().saturating_sub(rows);
    let end = available_lines.saturating_sub(offset);
    let start = end.saturating_sub(limit.clamp(1, NEW_LINES_MAX_ROWS));
    let candidates: Vec<_> = vt.lines().skip(start).take(end - start).collect();
    history_page(
        candidates
            .into_iter()
            .map(|line| render_history_line(line, ansi)),
        available_lines,
        offset,
        "terminal",
    )
}

/// capture-pane 已按视觉行分隔；逐行解析避免 pane 宽度变化或宽字符触发再次折行。
/// 仅重建 avt 确认的字符与 SGR，OSC/查询等原始序列不会直接回传。
pub(crate) fn captured_history(
    captured: &[&str],
    available_lines: usize,
    offset: usize,
    ansi: bool,
) -> HistoryPage {
    let mut pen = Pen::default();
    // capture-pane -e 的 SGR 可跨行沿用。必须先按旧→新解析，再反向选择输出预算。
    let rendered: Vec<_> = captured
        .iter()
        .map(|raw| {
            let cols = raw
                .chars()
                .count()
                .saturating_mul(2)
                .clamp(1, NEW_LINES_MAX_CHARS);
            let mut vt = Vt::builder().size(cols, 2).scrollback_limit(0).build();
            vt.feed_str(&sgr(pen));
            let mut clipped_line = None;
            for ch in raw.chars() {
                vt.feed(ch);
                if clipped_line.is_none() && vt.cursor().row > 0 {
                    clipped_line =
                        Some(render_history_line(vt.view().next().expect("第一行存在"), ansi).0);
                    // 保存前缀后继续消费尾部样式；关闭自动折行，避免超长行产生回滚分配。
                    vt.feed_str("\x1b[2;1H\x1b[?7l");
                }
            }
            let rendered = clipped_line.map(|line| (line, true)).unwrap_or_else(|| {
                render_history_line(vt.view().next().expect("第一行存在"), ansi)
            });
            // avt 未公开当前 Pen：在独立第二行写入哨兵，从其单元格取得已解析样式。
            // 下一行仅注入 sgr(Pen)，不沿用任何原始终端控制序列。
            vt.feed_str("\x1b[2;1H ");
            pen = *vt.view().nth(1).expect("第二行存在").cells()[0].pen();
            rendered
        })
        .collect();
    history_page(rendered.into_iter(), available_lines, offset, "tmux")
}

fn render_history_line(line: &Line, ansi: bool) -> (String, bool) {
    if ansi {
        render_ansi_line(line, NEW_LINES_MAX_CHARS - 1)
    } else {
        let text = line.text().trim_end().to_string();
        let mut end = text.len().min(NEW_LINES_MAX_CHARS - 1);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (text[..end].to_string(), end < text.len())
    }
}

fn history_page(
    candidates: impl DoubleEndedIterator<Item = (String, bool)>,
    available_lines: usize,
    offset: usize,
    source: &'static str,
) -> HistoryPage {
    let end = available_lines.saturating_sub(offset);
    let mut lines = Vec::new();
    let mut bytes = 0;
    let mut clipped = false;
    for (text, line_clipped) in candidates.rev() {
        if !lines.is_empty() && bytes + text.len() + 1 > NEW_LINES_MAX_CHARS {
            break;
        }
        bytes += text.len() + 1;
        lines.push(text);
        clipped |= line_clipped;
    }
    lines.reverse();
    let next_offset = (end > lines.len()).then(|| offset + lines.len());
    HistoryPage {
        source,
        lines,
        available_lines,
        next_offset,
        truncated: clipped || next_offset.is_some(),
    }
}

pub(super) fn ansi_line(line: &Line) -> String {
    render_ansi_line(line, usize::MAX).0
}

fn render_ansi_line(line: &Line, max_bytes: usize) -> (String, bool) {
    // 保留有背景/属性的行尾空格；只去掉真正没有视觉信息的默认 padding。
    let end = line
        .cells()
        .iter()
        .rposition(|cell| !cell.is_default())
        .map_or(0, |index| index + 1);
    let mut out = String::from("\x1b[0m");
    let mut pen = Pen::default();
    let mut clipped = false;
    for cell in line.cells()[..end].iter().filter(|cell| cell.width() > 0) {
        let style = if cell.pen() != &pen {
            sgr(*cell.pen())
        } else {
            String::new()
        };
        if out.len() + style.len() + cell.char().len_utf8() + 4 > max_bytes {
            clipped = true;
            break;
        }
        if !style.is_empty() {
            pen = *cell.pen();
            out.push_str(&style);
        }
        out.push(cell.char());
    }
    out.push_str("\x1b[0m");
    (out, clipped)
}

fn sgr(pen: Pen) -> String {
    let mut codes = vec![String::from("0")];
    for (enabled, code) in [
        (pen.is_bold(), "1"),
        (pen.is_faint(), "2"),
        (pen.is_italic(), "3"),
        (pen.is_underline(), "4"),
        (pen.is_blink(), "5"),
        (pen.is_inverse(), "7"),
        (pen.is_strikethrough(), "9"),
    ] {
        if enabled {
            codes.push(code.to_string());
        }
    }
    for (color, prefix) in [(pen.foreground(), "38"), (pen.background(), "48")] {
        match color {
            Some(Color::Indexed(index)) => codes.push(format!("{prefix};5;{index}")),
            Some(Color::RGB(rgb)) => {
                codes.push(format!("{prefix};2;{};{};{}", rgb.r, rgb.g, rgb.b));
            }
            None => {}
        }
    }
    format!("\x1b[{}m", codes.join(";"))
}
