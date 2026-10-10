use serde_json::{json, Value};

use super::{decode_escapes, parse_send_input};
use crate::ssh_tool::term::TEXT_MAX_CHARS;

fn args(text: &str) -> Value {
    json!({ "term_id": "term-test", "text": text, "intent": "交互测试" })
}

#[test]
fn user_reported_control_sequences_become_real_bytes() {
    let input = parse_send_input(&args(r"P\u0003Q\u0015R\u001bS")).unwrap();
    assert_eq!(input.text.as_bytes(), b"P\x03Q\x15R\x1bS");
}

#[test]
fn navigation_tab_enter_and_eof_decode_without_normalization() {
    let input = parse_send_input(&args(
        r"\u001b[A\e[B\x1b[C\u001b[D\e[5~\e[6~\t\r\n\r\n\u0004",
    ))
    .unwrap();
    assert_eq!(
        input.text.as_bytes(),
        b"\x1b[A\x1b[B\x1b[C\x1b[D\x1b[5~\x1b[6~\t\r\n\r\n\x04"
    );
}

#[test]
fn actual_controls_and_whitespace_are_preserved() {
    for text in ["\r", "\n", "\r\n", "\t", "  \u{3} 中 文 \u{1b}\t\n "] {
        assert_eq!(parse_send_input(&args(text)).unwrap().text, text);
    }
}

#[test]
fn literal_mode_preserves_shell_backslashes_and_actual_newlines() {
    for source in [
        "printf '%s\\n' '\\u0003' 'C:\\tmp'\n",
        "\\033\\r\\n\u{1b}\t\n\r",
    ] {
        let mut input = args(source);
        input["text_mode"] = json!("literal");
        assert_eq!(parse_send_input(&input).unwrap().text, source);
    }
}

#[test]
fn escaped_backslashes_are_not_recursively_decoded() {
    assert_eq!(decode_escapes(r"\\u0003").unwrap(), r"\u0003");
    assert_eq!(decode_escapes(r"\u005cu0003").unwrap(), r"\u0003");
    assert_eq!(decode_escapes(r"\\n").unwrap(), r"\n");
}

#[test]
fn unicode_and_surrogate_pairs_produce_utf8() {
    assert_eq!(
        decode_escapes(r"\u4e2d\u6587\ud83d\ude80").unwrap(),
        "中文🚀"
    );
}

#[test]
fn explicit_escaped_mode_has_the_same_default_behavior() {
    let mut input = args(r"\x03\e\t");
    input["text_mode"] = json!("escaped");
    assert_eq!(
        parse_send_input(&input).unwrap().text.as_bytes(),
        b"\x03\x1b\t"
    );
}

#[test]
fn octal_escapes_match_hex_unicode_and_short_controls() {
    for source in [r"\033[A", r"\x1b[A", r"\u001b[A", r"\e[A"] {
        assert_eq!(decode_escapes(source).unwrap().as_bytes(), b"\x1b[A");
    }
    assert_eq!(
        decode_escapes(r"\0\00\000\3\03\003\33\033\177\0001\08")
            .unwrap()
            .as_bytes(),
        b"\0\0\0\x03\x03\x03\x1b\x1b\x7f\x001\x008"
    );
    assert_eq!(decode_escapes(r"\\033").unwrap(), r"\033");
}

#[test]
fn enter_appends_cr_after_decoding_without_changing_literal_semantics() {
    for (source, mode, expected) in [
        ("echo ready", "escaped", "echo ready\r"),
        (r"echo ready\r", "escaped", "echo ready\r"),
        (r"echo ready\n", "escaped", "echo ready\n"),
        (r"echo ready\r\n", "escaped", "echo ready\r\n"),
        (r"echo ready\015", "escaped", "echo ready\r"),
        (r"echo ready\r", "literal", "echo ready\\r\r"),
        ("echo ready\r", "literal", "echo ready\r"),
        ("echo ready\n", "literal", "echo ready\n"),
        ("", "literal", "\r"),
    ] {
        let mut input = args(source);
        input["text_mode"] = json!(mode);
        input["enter"] = json!(true);
        assert_eq!(parse_send_input(&input).unwrap().text, expected);
    }
}

#[test]
fn omitted_or_false_enter_does_not_add_a_keystroke() {
    for mode in ["escaped", "literal"] {
        let mut input = args("echo ready");
        input["text_mode"] = json!(mode);
        assert_eq!(parse_send_input(&input).unwrap().text, "echo ready");
        input["enter"] = json!(false);
        assert_eq!(parse_send_input(&input).unwrap().text, "echo ready");
    }
}

#[test]
fn enter_rejects_non_boolean_values() {
    for enter in [json!("true"), json!(1), json!(0), json!({}), Value::Null] {
        let mut input = args("echo ready");
        input["enter"] = enter;
        assert!(parse_send_input(&input).is_err());
    }
}

#[test]
fn appended_enter_is_included_in_the_send_limit() {
    let mut input = args(&"中".repeat(TEXT_MAX_CHARS));
    input["enter"] = json!(true);
    assert!(parse_send_input(&input).is_err());
    input["text"] = json!("中".repeat(TEXT_MAX_CHARS - 1));
    assert_eq!(
        parse_send_input(&input).unwrap().text.chars().count(),
        TEXT_MAX_CHARS
    );
    input["text"] = json!(format!("{}\n", "中".repeat(TEXT_MAX_CHARS - 1)));
    assert_eq!(
        parse_send_input(&input).unwrap().text.chars().count(),
        TEXT_MAX_CHARS
    );
}

#[test]
fn all_supported_short_escapes_have_exact_ascii_bytes() {
    assert_eq!(
        decode_escapes(r#"\0\a\b\f\v\x7f\\\"\'\/"#)
            .unwrap()
            .as_bytes(),
        b"\x00\x07\x08\x0c\x0b\x7f\\\"'/"
    );
}

#[test]
fn malformed_and_unknown_escapes_fail_loudly() {
    for source in [
        "\\",
        r"\q",
        r"\x",
        r"\x1",
        r"\xgg",
        r"\xff",
        r"\x80",
        r"\200",
        r"\377",
        r"\400",
        r"\777",
        r"\8",
        r"\u",
        r"\u123",
        r"\uZZZZ",
        r"\ud800",
        r"\udc00",
        r"\ud800\u0000",
        r"\ud800x",
        r"\ud800\x00",
    ] {
        let error = decode_escapes(source).unwrap_err();
        assert!(error.contains("text_mode=literal"), "{source:?}: {error}");
    }
}

#[test]
fn invalid_mode_and_non_string_text_are_rejected() {
    for mode in [json!("raw"), json!("unknown"), json!(true), Value::Null] {
        let mut input = args("hello");
        input["text_mode"] = mode;
        assert!(parse_send_input(&input).is_err());
    }
    assert!(parse_send_input(&json!({ "term_id": "t", "text": 42, "intent": "测试" })).is_err());
}

#[test]
fn text_limit_counts_decoded_unicode_characters_and_never_truncates() {
    let source = r"\u4e2d".repeat(TEXT_MAX_CHARS);
    let input = parse_send_input(&args(&source)).unwrap();
    assert_eq!(input.text, "中".repeat(TEXT_MAX_CHARS));
    assert!(parse_send_input(&args(&format!("{source}X"))).is_err());
    let mut literal = args(&"🚀".repeat(TEXT_MAX_CHARS));
    literal["text_mode"] = json!("literal");
    assert_eq!(
        parse_send_input(&literal).unwrap().text.chars().count(),
        TEXT_MAX_CHARS
    );
    literal["text"] = json!("🚀".repeat(TEXT_MAX_CHARS + 1));
    assert!(parse_send_input(&literal).is_err());
}

#[test]
fn paste_wraps_literal_multiline_text_and_places_enter_outside_the_envelope() {
    let script = "cat <<'EOF'\nprintf '\\033[31mRED\\033[0m\\n'\nEOF\n";
    let mut input = args(script);
    input["text_mode"] = json!("literal");
    input["paste"] = json!(true);
    let parsed = parse_send_input(&input).unwrap();
    assert!(parsed.paste);
    assert_eq!(parsed.text, format!("\x1b[200~{script}\x1b[201~"));
    input["enter"] = json!(true);
    assert_eq!(
        parse_send_input(&input).unwrap().text,
        format!("\x1b[200~{script}\x1b[201~\r")
    );
}

#[test]
fn paste_decodes_controls_once_and_rejects_envelope_escape() {
    let mut input = args(r"\033[31mRED\033[0m\n");
    input["paste"] = json!(true);
    assert_eq!(
        parse_send_input(&input).unwrap().text,
        "\x1b[200~\x1b[31mRED\x1b[0m\n\x1b[201~"
    );
    for marker in [r"\033[201~", r"\e[200~", r"\u009b201~", r"\x1b[201~"] {
        input["text"] = json!(format!("before{marker}after"));
        assert!(parse_send_input(&input).is_err(), "{marker}");
    }
    input["text_mode"] = json!("literal");
    for marker in ["\x1b[200~", "\x1b[201~", "\u{9b}200~", "\u{9b}201~"] {
        input["text"] = json!(marker);
        assert!(parse_send_input(&input).is_err(), "{marker:?}");
    }
    // 字面反斜杠不是包络终止字节，仍可正常粘贴 shell 的转义表达式。
    input["text"] = json!(r"printf '\033[201~'");
    assert!(parse_send_input(&input).is_ok());
}

#[test]
fn paste_envelope_and_enter_share_the_reviewed_text_budget() {
    let mut input = args(&"中".repeat(TEXT_MAX_CHARS - 13));
    input["paste"] = json!(true);
    input["enter"] = json!(true);
    assert_eq!(
        parse_send_input(&input).unwrap().text.chars().count(),
        TEXT_MAX_CHARS
    );
    input["text"] = json!("中".repeat(TEXT_MAX_CHARS - 12));
    assert!(parse_send_input(&input).is_err());
    input["enter"] = json!(false);
    assert_eq!(
        parse_send_input(&input).unwrap().text.chars().count(),
        TEXT_MAX_CHARS
    );
}

#[test]
fn paste_defaults_off_and_rejects_non_boolean_values() {
    let mut input = args("hello");
    assert!(!parse_send_input(&input).unwrap().paste);
    for paste in [json!("true"), json!(1), Value::Null] {
        input["paste"] = paste;
        assert!(parse_send_input(&input).is_err());
    }
}

#[test]
fn intent_limit_counts_unicode_characters_and_requires_a_purpose() {
    let mut input = args("hello");
    input["intent"] = json!("中".repeat(200));
    assert_eq!(
        parse_send_input(&input).unwrap().intent.chars().count(),
        200
    );
    input["intent"] = json!("中".repeat(201));
    assert!(parse_send_input(&input).is_err());
    input["intent"] = json!("  \n\t ");
    assert!(parse_send_input(&input).is_err());
}
