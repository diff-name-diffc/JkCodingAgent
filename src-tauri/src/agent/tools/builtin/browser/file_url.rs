//! `file://` URL 解析与工作区拘禁校验。
//!
//! file: URL 可被用来读取任意本地文件（随后 browser_read_text 会把内容读入
//! 上下文），属于高危面：解析出本地路径后必须校验其位于当前工作区内，
//! 越界直接拒绝，不受 restrict_to_workspace 全局开关影响（fail-closed）。

use crate::agent::tools::builtin::common::canonicalize_existing_prefix;
use crate::agent::tools::context::ToolContext;

/// 解析 `file:` URL 中的本地路径部分。同时覆盖 `file:///path`、`file://localhost/path`
/// 与单斜杠 `file:/path`（RFC 8089 合法形式，浏览器引擎会归一化为三斜杠）。
/// 仅接受空主机或 `localhost`；其他主机（`file://host/...`）无法在本地校验，直接拒绝。
pub(super) fn file_url_to_path(url: &str) -> Result<std::path::PathBuf, String> {
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("错误：不是有效的 file:// URL".to_string());
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err("错误：不是有效的 file:// URL".to_string());
    }
    let path = match rest.strip_prefix("//") {
        Some(after_authority) => {
            // rest 形如 `[host]/path`；host 与 path 以第一个 `/` 分隔。
            let Some(slash_index) = after_authority.find('/') else {
                return Err(format!("错误：file:// URL 缺少本地路径：{url}"));
            };
            let host = &after_authority[..slash_index];
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return Err(format!("错误：file:// URL 不支持非本地主机：{host}"));
            }
            &after_authority[slash_index..]
        }
        // 单斜杠 `file:/path`（无 authority）。
        None => rest,
    };
    if path.is_empty() {
        return Err(format!("错误：file:// URL 缺少本地路径：{url}"));
    }
    Ok(std::path::PathBuf::from(percent_decode(path)))
}

/// 解码 URL 百分号转义（如 `%20` -> 空格），避免编码后的路径绕过校验。
/// 纯字节处理：先校验 `%` 后两字节均为 ASCII 十六进制位再解码，
/// 避免在多字节 UTF-8 字符上做 `&str` 切片导致 panic。
pub(super) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            // 已确保为 ASCII 十六进制位，from_utf8 必然成功。
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}

/// 校验 `file://` URL 指向的本地路径必须位于当前工作区内。
/// 通过 canonicalize（解析符号链接）后做前缀包含判断，防止 `..` 或符号链接逃逸。
pub(super) fn validate_file_url_within_workspace(
    url: &str,
    context: &ToolContext,
) -> Result<(), String> {
    let path = file_url_to_path(url)?;
    let candidate = canonicalize_existing_prefix(&path)
        .map_err(|error| format!("错误：无法解析 file:// URL 路径：{error}"))?;
    let workspace = context
        .workspace
        .canonicalize()
        .map_err(|error| format!("错误：解析工作区路径失败：{error}"))?;
    if !candidate.starts_with(&workspace) {
        return Err(format!(
            "错误：file:// URL 指向工作区之外的路径，已拒绝打开：{url}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{file_url_to_path, percent_decode};

    #[test]
    fn file_url_to_path_parses_local_paths() {
        assert_eq!(
            file_url_to_path("file:///etc/passwd").unwrap(),
            std::path::PathBuf::from("/etc/passwd")
        );
        assert_eq!(
            file_url_to_path("file:///Users/jk/a.png").unwrap(),
            std::path::PathBuf::from("/Users/jk/a.png")
        );
        assert_eq!(
            file_url_to_path("file://localhost/tmp/x.html").unwrap(),
            std::path::PathBuf::from("/tmp/x.html")
        );
        // 单斜杠形式（RFC 8089）同样必须被解析并纳入校验
        assert_eq!(
            file_url_to_path("file:/etc/passwd").unwrap(),
            std::path::PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn file_url_to_path_decodes_percent_escapes() {
        assert_eq!(
            file_url_to_path("file:///dir/file%20name.png").unwrap(),
            std::path::PathBuf::from("/dir/file name.png")
        );
    }

    #[test]
    fn file_url_to_path_rejects_remote_hosts_and_missing_path() {
        assert!(file_url_to_path("file://remote/share/x").is_err());
        assert!(file_url_to_path("file://localhost").is_err());
        assert!(file_url_to_path("file://").is_err());
        assert!(file_url_to_path("https://example.com").is_err());
    }

    #[test]
    fn percent_decode_handles_utf8_and_leaves_plus_literal() {
        assert_eq!(percent_decode("a%20b"), "a b");
        // UTF-8 多字节序列（中 = E4 B8 AD）
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        // 文件路径中 `+` 是字面量，不应解码为空格
        assert_eq!(percent_decode("a+b"), "a+b");
        // 非法转义按原样保留
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }
}
