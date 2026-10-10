//! avt 只维护屏幕；此处补齐受限的终端协议应答与粘贴模式跟踪。
//! 协议依据：https://invisible-island.net/xterm/ctlseqs/ctlseqs.html

const CSI_MAX_BYTES: usize = 32;
const PENDING_MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy)]
enum ParseState {
    Ground,
    Escape,
    Csi,
    DiscardCsi,
    ControlString { osc: bool, escaped: bool },
}

/// 解析状态跨输出帧保存；CSI 固定缓冲、控制字符串不缓存内容。
/// 仅生成协议应答并跟踪输入模式，绝不回答密码、主机指纹或确认提示。
pub(crate) struct TerminalResponses {
    state: ParseState,
    csi: [u8; CSI_MAX_BYTES],
    csi_len: usize,
    pending: Vec<u8>,
    bracketed_paste: bool,
    saved_bracketed_paste: bool,
}

impl TerminalResponses {
    pub(crate) fn new() -> Self {
        Self {
            state: ParseState::Ground,
            csi: [0; CSI_MAX_BYTES],
            csi_len: 0,
            pending: Vec::new(),
            bracketed_paste: false,
            saved_bracketed_paste: false,
        }
    }

    /// 每个字符先交给 avt，再传入该时刻的零基光标和 `(cols, rows)`。
    /// 不可用帧末坐标批量回答：同一帧可能包含多次移动和查询。
    /// 调用者每帧取走 pending；应答洪泛超过上限时必须向会话暴露错误。
    pub(crate) fn feed(
        &mut self,
        ch: char,
        cursor: (usize, usize),
        size: (usize, usize),
    ) -> Result<(), String> {
        if matches!(ch, '\u{18}' | '\u{1a}') {
            self.state = ParseState::Ground;
            return Ok(());
        }
        if let ParseState::ControlString { osc, escaped } = self.state {
            self.state = if ch == '\u{9c}' || (escaped && ch == '\\') || (osc && ch == '\x07') {
                ParseState::Ground
            } else {
                // 字符串内的 ESC [ 不开启 CSI；避免标题、DCS 载荷触发回写。
                ParseState::ControlString {
                    osc,
                    escaped: ch == '\x1b',
                }
            };
            return Ok(());
        }
        match ch {
            '\x1b' => {
                self.state = ParseState::Escape;
                return Ok(());
            }
            '\u{9b}' => {
                self.start_csi();
                return Ok(());
            }
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                self.start_string(ch == '\u{9d}');
                return Ok(());
            }
            _ => {}
        }
        match self.state {
            ParseState::Ground => {}
            ParseState::Escape => match ch {
                '[' => self.start_csi(),
                ']' => self.start_string(true),
                'P' | 'X' | '^' | '_' => self.start_string(false),
                'c' => {
                    self.bracketed_paste = false;
                    self.saved_bracketed_paste = false;
                    self.state = ParseState::Ground;
                }
                // 未支持的 ESC 中间字符序列不可误认成 ESC [。
                '\x00'..='\x1f' => {}
                _ => self.state = ParseState::Ground,
            },
            ParseState::Csi => match ch {
                '\x40'..='\x7e' => {
                    self.state = ParseState::Ground;
                    self.track_paste_mode(ch);
                    if let Some(reply) = self.reply(ch, cursor, size)? {
                        self.enqueue(reply.as_bytes())?;
                    }
                }
                '\x20'..='\x3f' => {
                    if self.csi_len == self.csi.len() {
                        self.state = ParseState::DiscardCsi;
                    } else {
                        self.csi[self.csi_len] = ch as u8;
                        self.csi_len += 1;
                    }
                }
                '\x00'..='\x1f' | '\x7f' => {}
                _ => self.state = ParseState::Ground,
            },
            ParseState::DiscardCsi => {
                if ('\x40'..='\x7e').contains(&ch) {
                    self.state = ParseState::Ground;
                }
            }
            ParseState::ControlString { .. } => unreachable!("控制字符串已在入口处理"),
        }
        Ok(())
    }

    pub(crate) fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    fn track_paste_mode(&mut self, final_byte: char) {
        let Some(params) = self.csi[..self.csi_len].strip_prefix(b"?") else {
            return;
        };
        if !params
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            return;
        }
        if !params.split(|byte| *byte == b';').any(|param| {
            std::str::from_utf8(param)
                .ok()
                .and_then(|param| param.parse::<u16>().ok())
                == Some(2004)
        }) {
            return;
        }
        match final_byte {
            'h' => self.bracketed_paste = true,
            'l' => self.bracketed_paste = false,
            's' => self.saved_bracketed_paste = self.bracketed_paste,
            'r' => self.bracketed_paste = self.saved_bracketed_paste,
            _ => {}
        }
    }

    fn start_csi(&mut self) {
        self.state = ParseState::Csi;
        self.csi_len = 0;
    }

    fn start_string(&mut self, osc: bool) {
        self.state = ParseState::ControlString {
            osc,
            escaped: false,
        };
    }

    fn reply(
        &self,
        final_byte: char,
        cursor: (usize, usize),
        size: (usize, usize),
    ) -> Result<Option<String>, String> {
        let params = &self.csi[..self.csi_len];
        let reply = match (params, final_byte) {
            (b"5", 'n') => "\x1b[0n".to_string(),
            (b"6" | b"?6", 'n') => {
                let (row, col) = cursor;
                let (cols, rows) = size;
                if cols == 0 || row >= rows || col > cols {
                    return Err("错误：终端光标超出屏幕范围，无法回答 CPR 查询".into());
                }
                // avt 以 col == cols 表示行末待换行，真实光标仍停在最后一列。
                let col = col.min(cols - 1);
                let private = if params[0] == b'?' { "?" } else { "" };
                format!("\x1b[{private}{};{}R", row + 1, col + 1)
            }
            // VT100 + 基础视觉属性，不宣称未实现的 Sixel、剪贴板等能力。
            (b"" | b"0", 'c') => "\x1b[?1;2c".to_string(),
            (b"18", 't') => {
                let (cols, rows) = size;
                if cols == 0 || rows == 0 {
                    return Err("错误：终端尺寸为空，无法回答字符尺寸查询".into());
                }
                format!("\x1b[8;{rows};{cols}t")
            }
            // DECTCEM (?25h/l) 是设置命令；应答它会将无端输入注入远端。
            _ => return Ok(None),
        };
        Ok(Some(reply))
    }

    fn enqueue(&mut self, reply: &[u8]) -> Result<(), String> {
        if self.pending.len() + reply.len() > PENDING_MAX_BYTES {
            return Err(format!(
                "错误：单帧终端查询应答超过 {PENDING_MAX_BYTES} 字节，拒绝继续自动应答"
            ));
        }
        self.pending.extend_from_slice(reply);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(responses: &mut TerminalResponses, text: &str) -> Result<(), String> {
        for ch in text.chars() {
            responses.feed(ch, (2, 6), (80, 24))?;
        }
        Ok(())
    }

    #[test]
    fn bracketed_paste_tracks_chunked_modes_save_restore_and_reset() {
        let mut responses = TerminalResponses::new();
        assert!(!responses.bracketed_paste());
        for chunk in ["\x1b[?20", "04", "h"] {
            feed(&mut responses, chunk).unwrap();
        }
        assert!(responses.bracketed_paste());
        feed(&mut responses, "\x1b[?2004s\x1b[?25;2004l").unwrap();
        assert!(!responses.bracketed_paste());
        feed(&mut responses, "\x1b[?2004r").unwrap();
        assert!(responses.bracketed_paste());
        feed(&mut responses, "\x1bc\x1b[?2004r").unwrap();
        assert!(!responses.bracketed_paste());
        feed(&mut responses, "\u{9b}?02004h").unwrap();
        assert!(responses.bracketed_paste());
        assert!(responses.take_pending().is_empty());
    }

    #[test]
    fn bracketed_paste_ignores_control_strings_and_malformed_modes() {
        let mut responses = TerminalResponses::new();
        for sequence in [
            "\x1b]title \x1b[?2004h\x07",
            "\x1bPdata \x1b[?2004h\x1b\\",
            "\x1b[?2004;25 h",
            "\x1b[?2004;2:3h",
            "\x1b[?2004\x18h",
            "\x1b[2004h",
        ] {
            feed(&mut responses, sequence).unwrap();
            assert!(!responses.bracketed_paste(), "{sequence:?}");
        }
    }

    #[test]
    fn answers_supported_queries_and_keeps_chunk_state() {
        let mut responses = TerminalResponses::new();
        for chunk in [
            "\x1b",
            "[",
            "6",
            "n\x1b[5",
            "n\x1b[c\x1b[0c\x1b[18t\x1b[?6n",
        ] {
            feed(&mut responses, chunk).unwrap();
        }
        assert_eq!(
            responses.take_pending(),
            b"\x1b[3;7R\x1b[0n\x1b[?1;2c\x1b[?1;2c\x1b[8;24;80t\x1b[?3;7R"
        );
        assert!(responses.take_pending().is_empty());
    }

    #[test]
    fn replies_use_each_query_time_cursor_and_current_size() {
        let mut responses = TerminalResponses::new();
        feed(&mut responses, "\x1b[6").unwrap();
        responses.feed('n', (0, 0), (80, 24)).unwrap();
        feed(&mut responses, "\x1b[6").unwrap();
        responses.feed('n', (29, 119), (120, 30)).unwrap();
        feed(&mut responses, "\x1b[18").unwrap();
        responses.feed('t', (0, 0), (120, 30)).unwrap();
        assert_eq!(
            responses.take_pending(),
            b"\x1b[1;1R\x1b[30;120R\x1b[8;30;120t"
        );
    }

    #[test]
    fn ignores_control_strings_until_their_terminator() {
        let mut responses = TerminalResponses::new();
        for introducer in ["\x1b]", "\x1bP", "\x1bX", "\x1b^", "\x1b_", "\u{9d}"] {
            feed(&mut responses, introducer).unwrap();
            feed(&mut responses, "title \x1b[6n\x1b[c\x1b").unwrap();
            feed(&mut responses, "\\\x1b[5n").unwrap();
            assert_eq!(responses.take_pending(), b"\x1b[0n");
        }
        feed(&mut responses, "\x1b]title\x07\x1b[5n").unwrap();
        assert_eq!(responses.take_pending(), b"\x1b[0n");
        feed(&mut responses, "\x1bPignored\x07\x1b[6n\u{9c}\x1b[5n").unwrap();
        assert_eq!(responses.take_pending(), b"\x1b[0n");
    }

    #[test]
    fn ignores_modes_unsupported_queries_and_echoed_replies() {
        let mut responses = TerminalResponses::new();
        feed(
            &mut responses,
            "\x1b[?25h\x1b[?25l\x1b[>c\x1b[1c\x1b[14t\x1b[6$n\x1b[?1;2c\x1b[0n\x1b[3;7R",
        )
        .unwrap();
        assert!(responses.take_pending().is_empty());
    }

    #[test]
    fn bounded_csi_recovers_after_overlong_or_cancelled_sequences() {
        let mut responses = TerminalResponses::new();
        feed(&mut responses, &format!("\x1b[{}n", "6".repeat(100_000))).unwrap();
        assert_eq!(responses.csi_len, CSI_MAX_BYTES);
        feed(&mut responses, "\x1b[6\x18n\x1b[6\x1an\x1b[5n").unwrap();
        assert_eq!(responses.take_pending(), b"\x1b[0n");
    }

    #[test]
    fn reply_flood_fails_explicitly_and_never_exceeds_limit() {
        let mut responses = TerminalResponses::new();
        let error = feed(&mut responses, &"\x1b[5n".repeat(PENDING_MAX_BYTES)).unwrap_err();
        assert!(error.contains("拒绝继续自动应答"));
        assert_eq!(responses.pending.len(), PENDING_MAX_BYTES);
    }

    #[test]
    fn invalid_screen_state_is_reported() {
        let mut responses = TerminalResponses::new();
        feed(&mut responses, "\x1b[6").unwrap();
        assert!(responses.feed('n', (24, 0), (80, 24)).is_err());
        feed(&mut responses, "\x1b[18").unwrap();
        assert!(responses.feed('t', (0, 0), (0, 24)).is_err());
    }

    #[test]
    fn pending_wrap_reports_last_visible_column() {
        let mut responses = TerminalResponses::new();
        feed(&mut responses, "\x1b[6").unwrap();
        responses.feed('n', (0, 80), (80, 24)).unwrap();
        assert_eq!(responses.take_pending(), b"\x1b[1;80R");
    }
}
