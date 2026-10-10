//! 终端屏幕模型胶水（avt）：UTF-8 拼帧、备用屏探测、双轨读屏。
//!
//! 双轨语义（设计文档 §4.3/§5.4）：
//! - 增量轨（newLines）采用**光标行定稿算法**：光标推进到新行时，上一行文本定稿
//!   入队；同行重写（`\r` 进度条、退格编辑）不重复产出，最终形态在定稿时进入。
//!   相比设计初稿的「行数不变量」，该算法对不满屏输出同样产出增量——短命令输出
//!   不触发滚动，行数不变量下 newLines 恒空，增量语义失效。
//! - 快照轨（screen）：avt 活动缓冲区的可见行（`view()`；备用屏下即全屏程序
//!   渲染结果，禁止用 `text()`——它恒取主缓冲）。
//! - 备用屏的 newLines 是两次读取之间新增/改写的可见行，忽略仅发生位置移动的
//!   旧行；不是完整输出日志，读取间已经滚出的内容仍可能丢失。删除/清屏以快照
//!   为准。退出备用屏时以当前光标行重新对齐。

use std::collections::VecDeque;

use super::responses::TerminalResponses;
use avt::Vt;

mod delta;
mod rendering;

pub(super) use rendering::captured_history;
pub(crate) use rendering::HistoryPage;

pub(crate) const DEFAULT_COLS: usize = 80;
pub(crate) const DEFAULT_ROWS: usize = 24;
const SCROLLBACK_LIMIT: usize = 1000;

/// 增量轨单次 read 的上限（行数 / 字符数，命令类口径），超限置 truncated，
/// 剩余部分留队由后续 read 补齐。
pub(crate) const NEW_LINES_MAX_ROWS: usize = 200;
pub(crate) const NEW_LINES_MAX_CHARS: usize = 12_000;

/// 增量轨内存上限：模型长时间不 read 时的防膨胀闸门，超限丢弃最旧行。
const PENDING_QUEUE_MAX_ROWS: usize = 2_000;

/// UTF-8 拼帧器：channel 给字节、avt 吃 `&str`，多字节序列可能跨包切断，
/// pending 最多残留一个未完成字符的字节前缀（≤4）。
pub(crate) struct Utf8Framer {
    pending: Vec<u8>,
}

impl Utf8Framer {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::with_capacity(4),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(_) => {
                    out.push_str(&String::from_utf8_lossy(&self.pending));
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match e.error_len() {
                        // 确认非法的字节：替换后继续处理余下数据。
                        Some(len) => {
                            self.pending.drain(..valid + len);
                            out.push('\u{fffd}');
                        }
                        // 尾部是未完成的多字节前缀：保留待下包。
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }
}

/// 备用屏进入/退出序列（xterm 私有模式 1049/1047/47 的 set/reset）。
const ENTER_SEQS: [&str; 3] = ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h"];
const EXIT_SEQS: [&str; 3] = ["\x1b[?1049l", "\x1b[?1047l", "\x1b[?47l"];
/// 转义序列可能被分包切断，扫描保留尾部 ≤8 字节拼接到下一帧再匹配。
const CARRY_MAX_BYTES: usize = 8;

pub(crate) enum AltScreenEvent {
    /// 进入备用屏（全屏程序启动 / tmux client attach）。
    Entered,
    /// 退回主缓冲。
    Left,
}

/// 备用屏探测：avt 的 `Vt` 未公开 `active_buffer_type`，在拼帧输出上做
/// 字节级扫描的状态机补偿（设计文档 §5.3）。
pub(crate) struct AltScreenTracker {
    on_alt: bool,
    carry: String,
}

impl AltScreenTracker {
    pub(crate) fn new() -> Self {
        Self {
            on_alt: false,
            carry: String::new(),
        }
    }

    pub(crate) fn is_alt(&self) -> bool {
        self.on_alt
    }

    /// 喂入一帧文本；仅当备用屏状态真实翻转时返回事件。
    pub(crate) fn push(&mut self, s: &str) -> Option<AltScreenEvent> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.push_str(s);
        // 一帧内最晚命中的序列决定帧末状态（连续进出取最终）。
        let mut hit: Option<(usize, bool)> = None;
        for (seqs, enter) in [(&ENTER_SEQS, true), (&EXIT_SEQS, false)] {
            for seq in seqs {
                if let Some(pos) = buf.rfind(seq) {
                    let end = pos + seq.len();
                    if hit.is_none() || end > hit.expect("checked").0 {
                        hit = Some((end, enter));
                    }
                }
            }
        }
        // 终端复位（RIS，ESC c）回到主屏：视为退出备用屏事件参与最晚判定。
        if let Some(pos) = buf.rfind("\x1bc") {
            let end = pos + "\x1bc".len();
            if hit.is_none() || end > hit.expect("checked").0 {
                hit = Some((end, false));
            }
        }
        let event = match hit {
            Some((_, enter)) if enter != self.on_alt => {
                self.on_alt = enter;
                Some(if enter {
                    AltScreenEvent::Entered
                } else {
                    AltScreenEvent::Left
                })
            }
            _ => None,
        };
        // 保留尾部可能被切断的序列前缀（按字符边界截，不破坏 UTF-8）。
        if buf.len() > CARRY_MAX_BYTES {
            let mut cut = buf.len() - CARRY_MAX_BYTES;
            while cut > 0 && !buf.is_char_boundary(cut) {
                cut -= 1;
            }
            self.carry = buf[cut..].to_string();
        } else {
            self.carry = buf;
        }
        event
    }
}

pub(crate) struct NewLinesBatch {
    pub(crate) lines: Vec<String>,
    pub(crate) truncated: bool,
}

/// 终端屏幕模型：avt `Vt` + 双轨游标。单线程拥有（由 `TermSession` 的锁保护），
/// 临界区内纯内存操作。
pub(crate) struct TermScreen {
    vt: Vt,
    framer: Utf8Framer,
    alt_tracker: AltScreenTracker,
    /// 历史上被裁出回滚上限的行数（行索引 → 绝对行号换算）。
    ejected_so_far: usize,
    /// 当前跟踪的绝对行号与最新文本（未定稿；定稿行进 `pending`）。
    cur_abs_row: usize,
    cur_line_text: String,
    pending: VecDeque<String>,
    /// 内存闸门触发过丢弃（永久信息损失），透传 truncated 标记。
    pending_overflow: bool,
    /// 备用屏期间暂停主屏行定稿，改用读取时的可见行差分。
    paused: bool,
    last_alt_view: Vec<String>,
    responses: TerminalResponses,
    response_error: Option<String>,
}

impl TermScreen {
    pub(crate) fn new(cols: usize, rows: usize) -> Self {
        let vt = Vt::builder()
            .size(cols, rows)
            .scrollback_limit(SCROLLBACK_LIMIT)
            .build();
        Self {
            vt,
            framer: Utf8Framer::new(),
            alt_tracker: AltScreenTracker::new(),
            ejected_so_far: 0,
            cur_abs_row: 0,
            cur_line_text: String::new(),
            pending: VecDeque::new(),
            pending_overflow: false,
            paused: false,
            last_alt_view: Vec::new(),
            responses: TerminalResponses::new(),
            response_error: None,
        }
    }

    /// 喂入 channel 原始字节：拼帧 → 备用屏探测 → avt 解析 → 增量轨跟踪。
    pub(crate) fn push_bytes(&mut self, chunk: &[u8]) {
        let s = self.framer.push(chunk);
        if s.is_empty() {
            return;
        }
        let alt_event = self.alt_tracker.push(&s);
        // 每次查询按出现当刻的光标回答；Vt::feed 不触发 GC，帧尾统一回收。
        for ch in s.chars() {
            self.vt.feed(ch);
            if self.response_error.is_none() {
                let cursor = self.vt.cursor();
                self.response_error = self
                    .responses
                    .feed(ch, (cursor.row, cursor.col), self.vt.size())
                    .err();
            }
        }
        // avt 未导出 Changes 类型名（vt 模块私有）：块作用域内取走 scrollback
        // 行文本，块尾 changes 整体 drop 后 &mut Vt 借用结束，才能继续借用 self。
        let scrollback_texts: Vec<String> = {
            let changes = self.vt.feed_str("");
            changes.scrollback.map(|line| line.text()).collect()
        };
        self.ejected_so_far += scrollback_texts.len();
        match alt_event {
            Some(AltScreenEvent::Entered) => {
                self.paused = true;
                self.cur_line_text.clear();
                self.last_alt_view.clear();
            }
            Some(AltScreenEvent::Left) => {
                self.paused = false;
                self.realign_to_cursor();
                self.last_alt_view.clear();
            }
            None => {}
        }
        if !self.paused {
            self.track_cursor_line(scrollback_texts);
        }
    }

    pub(crate) fn take_responses(&mut self) -> Result<Vec<u8>, String> {
        if let Some(error) = self.response_error.take() {
            return Err(error);
        }
        Ok(self.responses.take_pending())
    }

    pub(crate) fn validate_paste_state(&self) -> Result<(), String> {
        if self.responses.bracketed_paste() {
            Ok(())
        } else {
            Err("远端当前未启用 bracketed paste；本次未发送，也未降级为逐键输入。多行脚本请使用 ssh_exec 的 stdin（如 command=\"bash -s\"），或等待支持粘贴的 shell/编辑器就绪。".into())
        }
    }

    /// 增量轨取货：排空前 N 行（上限内），超限部分留队待后续 read。
    pub(crate) fn take_new_lines(&mut self) -> NewLinesBatch {
        if self.paused {
            let mut current: Vec<String> = self
                .vt
                .view()
                .map(|line| line.text().trim_end().to_string())
                .collect();
            while current.last().is_some_and(String::is_empty) {
                current.pop();
            }
            for line in delta::added_lines(&self.last_alt_view, &current) {
                self.enqueue_line(line);
            }
            self.last_alt_view = current;
        }
        let mut lines = Vec::new();
        let mut chars = 0usize;
        let mut truncated = self.pending_overflow;
        self.pending_overflow = false;
        while let Some(front) = self.pending.front() {
            let row_cap = lines.len() >= NEW_LINES_MAX_ROWS;
            let char_cap = !lines.is_empty() && chars + front.len() > NEW_LINES_MAX_CHARS;
            if row_cap || char_cap {
                truncated = true;
                break;
            }
            chars += front.len();
            lines.push(self.pending.pop_front().expect("front checked"));
        }
        NewLinesBatch { lines, truncated }
    }

    /// 快照轨：活动缓冲区可见行（含全屏程序）；尾随 padding 对流式行语义是
    /// 噪声、对快照无信息量，统一 trim。
    pub(crate) fn snapshot(&self) -> String {
        self.vt
            .view()
            .map(|line| line.text().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 从 avt 单元格样式重建 SGR 快照；不回传 OSC、终端查询等原始控制序列。
    pub(crate) fn snapshot_ansi(&self) -> String {
        self.vt
            .view()
            .map(rendering::ansi_line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// offset 从视口之前最新的历史行向前计数；每页按旧→新排列。
    /// 备用屏通常无回滚区，此接口不代表 tmux 服务端的历史。
    pub(crate) fn history(&self, offset: usize, limit: usize, ansi: bool) -> HistoryPage {
        rendering::history(&self.vt, offset, limit, ansi)
    }

    /// 等待模式在不消费增量队列的前提下同时观察已定稿输出与当前屏幕。
    pub(crate) fn matches_output(&self, pattern: &str) -> bool {
        // 两轨存在重叠，不能拼接制造从未出现过的跨轨文本。
        let pending = self
            .pending
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        pending.contains(pattern) || self.snapshot().contains(pattern)
    }

    pub(crate) fn cursor(&self) -> (usize, usize) {
        let cursor = self.vt.cursor();
        (cursor.row, cursor.col)
    }

    pub(crate) fn alt_screen(&self) -> bool {
        self.alt_tracker.is_alt()
    }

    /// 送审用的终端现场摘要：光标行 + 屏幕尾部非空行（≤5），供审查模型判断
    /// 按键片段的语境（设计文档 §4.2/§7——`Y\r` 单看不可判，配合屏幕上的
    /// 确认提示即可裁决）。来源侧截断，渲染层不再二次截断。
    pub(crate) fn screen_context(&self) -> String {
        const TAIL_LINES: usize = 5;
        const MAX_CHARS: usize = 1_200;
        let mut lines: Vec<String> = self
            .vt
            .view()
            .map(|line| line.text().trim_end().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        let start = lines.len().saturating_sub(TAIL_LINES);
        let tail = lines.split_off(start);
        let (row, _) = self.cursor();
        let cursor_line = self
            .vt
            .view()
            .nth(row)
            .map(|line| line.text().trim_end().to_string())
            .unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        let mut cursor_marked = false;
        for line in tail {
            // 光标行通常就是尾部行（提示符/确认提示在屏幕底部）：就地标注，
            // 保证审查模型始终能看出按键将落在哪一行。
            if !cursor_marked && !cursor_line.is_empty() && line == cursor_line {
                parts.push(format!("[光标行] {line}"));
                cursor_marked = true;
            } else {
                parts.push(line);
            }
        }
        if !cursor_marked && !cursor_line.is_empty() {
            parts.insert(0, format!("[光标行] {cursor_line}"));
        }
        let joined = parts.join("\n");
        if joined.chars().count() > MAX_CHARS {
            joined.chars().take(MAX_CHARS).collect()
        } else {
            joined
        }
    }

    /// 尺寸同步（与 russh window_change 两侧一致）。reflow 会重排行布局，
    /// 保守重对齐增量游标、丢弃未定稿行。
    pub(crate) fn resize(&mut self, cols: usize, rows: usize) {
        let ejected = {
            let changes = self.vt.resize(cols, rows);
            changes.scrollback.count()
        };
        self.ejected_so_far += ejected;
        if !self.paused {
            self.realign_to_cursor();
        } else {
            // 备用屏差分基线是旧布局：reflow 换行的旧行会被 LCS 差分误判为
            // 新增行，清空后下一次 read 按首次读取语义整体重报。
            self.last_alt_view.clear();
        }
    }

    /// 以当前光标行重新对齐增量轨（备屏退出 / resize 后）。
    fn realign_to_cursor(&mut self) {
        self.cur_abs_row = self.cursor_abs_row();
        self.cur_line_text = self.total_line_text(self.cur_abs_row);
    }

    /// 光标行定稿算法主体（见模块注释）。一帧推送可能推进多行（reader 的 Data
    /// 帧常含整段命令输出）：定稿 `[cur_abs_row, 光标绝对行)` 的全部行；滚出
    /// 回滚上限被裁的行从 `scrollback` 文本补定稿（绝对行号 < cur_abs_row 的
    /// 已定稿过，跳过）。
    ///
    /// 绝对行号 = 行出现的历史序号（0 起）。探针实证（avt 0.18）：滚动时光标
    /// 停在视口最后一行、行进回滚区但 `scrollback` 仅在超出回滚上限被裁时产出，
    /// 因此光标行号须按总行数换算：`abs = ejected + retained - rows + row`。
    fn track_cursor_line(&mut self, scrollback_texts: Vec<String>) {
        let ejected_before = self.ejected_so_far - scrollback_texts.len();
        for (offset, text) in scrollback_texts.into_iter().enumerate() {
            let abs_line = ejected_before + offset;
            if abs_line >= self.cur_abs_row {
                self.enqueue_line(text);
            }
        }
        let abs = self.cursor_abs_row();
        // 屏内补定稿：从已跟踪行（且未被裁出屏）到光标行的前一行。
        // 光标回跳（清屏 / 重绘）时 while 不执行，直接重置（快照轨可见）。
        let mut line_no = self.cur_abs_row.max(self.ejected_so_far);
        while line_no < abs {
            self.enqueue_line(self.total_line_text(line_no));
            line_no += 1;
        }
        self.cur_abs_row = abs;
        self.cur_line_text = self.total_line_text(abs);
    }

    /// 光标行的绝对行号：总行数不变量换算（`vt.line(n)` 为含回滚的总坐标）。
    fn cursor_abs_row(&self) -> usize {
        let (row, _) = self.cursor();
        let (_, rows) = self.vt.size();
        self.ejected_so_far + self.vt.lines().count() - rows + row
    }

    /// 总坐标行文本。注意 avt 坐标系二义性：`lines()` 迭代是总坐标（回滚+视口），
    /// 而 `vt.line(n)` 的 n 是视口坐标（`lines[view_offset + n]`，越界 panic）。
    fn total_line_text(&self, abs: usize) -> String {
        let idx = abs - self.ejected_so_far;
        self.vt
            .lines()
            .nth(idx)
            .map(|line| line.text().trim_end().to_string())
            .unwrap_or_default()
    }

    fn enqueue_line(&mut self, text: String) {
        if self.pending.len() >= PENDING_QUEUE_MAX_ROWS {
            self.pending.pop_front();
            self.pending_overflow = true;
        }
        self.pending.push_back(text);
    }
}

#[cfg(test)]
mod tests;
