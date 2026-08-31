//! 本机 `~/.ssh/config` 解析导入：把 Host 条目转换为 SSH 服务器配置草稿。
//!
//! 只读解析、不落库：草稿由设置页确认合并后随整体保存入库（凭据不导入）。
//! 从 `mod.rs` 拆出，收敛与连接执行无关的纯解析函数。

use super::types::{DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECS};
use super::validation::ID_MAX_LEN;
use super::{SshAuthMethod, SshServerConfig};

#[tauri::command]
pub async fn ssh_tool_import_ssh_config() -> Result<Vec<SshServerConfig>, String> {
    tokio::task::spawn_blocking(|| {
        let Some(home) = dirs::home_dir() else {
            return Err("无法确定用户主目录".to_string());
        };
        let path = home.join(".ssh").join("config");
        if !path.is_file() {
            return Err(format!("未找到 SSH 配置文件：{}", path.display()));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        Ok(parse_ssh_config(&content))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// OpenSSH client config 的极简解析：识别 Host 块与 HostName/Port/User/IdentityFile。
/// - 含通配符（`*`/`?`）或取反（`!`）的 Host 模式不生成条目；
/// - 仅当 Host 行恰为单个 `*` 模式时记为全局默认块，其 User/Port/IdentityFile
///   按关键字各自取所有全局默认块中首个非空值（OpenSSH「先命中优先」语义的
///   逐关键字近似）；`Host *.example.com` 这类模式块只匹配相应主机，不作全局默认源；
/// - 同一块内重复关键字只取第一次出现的值；
/// - 关键字支持 `Key value` 与 `Key=value` 两种写法，大小写不敏感；
/// - 值按双引号分词：`Host "my host"` 是单个别名；`IdentityFile key1 key2`
///   取第一个文件（OpenSSH 允许一行多个，其余等价于后续 IdentityFile 行）。
fn parse_ssh_config(content: &str) -> Vec<SshServerConfig> {
    #[derive(Default)]
    struct HostBlock {
        alias: Option<String>,
        /// Host 行恰为单个 `*` 模式（可作全局默认值来源）。
        global_default: bool,
        host_name: Option<String>,
        port: Option<u16>,
        user: Option<String>,
        identity_file: Option<String>,
    }

    let mut blocks: Vec<HostBlock> = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, value)) = split_config_directive(line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            // 一个 Host 行可带多个模式：取第一个具体别名；全是通配/取反模式则
            // 不生成条目。仅当该行恰为单个 `*` 时记为全局默认块。
            let patterns = split_ssh_tokens(value);
            let alias = patterns
                .iter()
                .find(|pattern| !pattern.starts_with('!') && !pattern.contains(['*', '?']));
            blocks.push(HostBlock {
                alias: alias.cloned(),
                global_default: patterns.len() == 1 && patterns[0] == "*",
                ..Default::default()
            });
            continue;
        }
        let Some(block) = blocks.last_mut() else {
            continue; // 文件开头、任何 Host 之前的全局指令忽略
        };
        // OpenSSH 语义：同一字段先出现的值生效。
        if keyword.eq_ignore_ascii_case("hostname") && block.host_name.is_none() {
            block.host_name = Some(unquote(value));
        } else if keyword.eq_ignore_ascii_case("port") && block.port.is_none() {
            block.port = value.trim().parse::<u16>().ok().filter(|port| *port >= 1);
        } else if keyword.eq_ignore_ascii_case("user") && block.user.is_none() {
            block.user = Some(unquote(value));
        } else if keyword.eq_ignore_ascii_case("identityfile") && block.identity_file.is_none() {
            // 一行多个文件时只取第一个（分词已剥离引号）。
            block.identity_file = split_ssh_tokens(value).into_iter().next();
        }
    }

    // 先命中优先：按文件顺序遍历全部 `Host *` 全局默认块，每个关键字各自取
    // 首个非空值（后续块只补前面块缺失的关键字）。
    let global_defaults = || blocks.iter().filter(|block| block.global_default);
    let default_port = global_defaults().find_map(|block| block.port);
    let fallback_user = global_defaults()
        .find_map(|block| block.user.clone())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_default();
    let fallback_identity = global_defaults()
        .find_map(|block| block.identity_file.clone())
        .or_else(default_identity_file);

    let mut used_ids = std::collections::HashSet::new();
    blocks
        .into_iter()
        .filter_map(|block| {
            let alias = block.alias?;
            let host = block
                .host_name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| alias.clone());
            let identity_file = block.identity_file.or_else(|| fallback_identity.clone());
            let (auth_method, private_key_path) = match identity_file {
                Some(path) if !path.trim().is_empty() => (SshAuthMethod::Key, path),
                _ => (SshAuthMethod::Password, String::new()),
            };
            let id = unique_server_id(&sanitize_server_id(&alias), &mut used_ids);
            Some(SshServerConfig {
                id,
                name: alias,
                enabled: true,
                host,
                port: block.port.or(default_port).unwrap_or(22),
                username: block.user.clone().unwrap_or_else(|| fallback_user.clone()),
                password: String::new(),
                auth_method,
                private_key_path,
                private_key_passphrase: String::new(),
                description: String::new(),
                tags: Vec::new(),
                review_enabled: true,
                default_timeout_secs: DEFAULT_TIMEOUT_SECS,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            })
        })
        .collect()
}

/// 把 `Key value` / `Key=value` 行拆成（关键字, 值）。
fn split_config_directive(line: &str) -> Option<(&str, &str)> {
    if let Some((key, value)) = line.split_once('=') {
        return Some((key.trim(), value.trim()));
    }
    let (key, value) = line.split_once(char::is_whitespace)?;
    Some((key, value.trim()))
}

/// 按空白分词，双引号内的空白不作为分隔符（引号本身被剥离）。
/// `Host "my host" other` → `["my host", "other"]`。
fn split_ssh_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;
    for ch in value.chars() {
        if ch == '"' {
            quoted = !quoted;
            has_token = true;
        } else if ch.is_whitespace() && !quoted {
            if has_token {
                tokens.push(std::mem::take(&mut current));
                has_token = false;
            }
        } else {
            current.push(ch);
            has_token = true;
        }
    }
    if has_token {
        tokens.push(current);
    }
    tokens
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

/// 探测 OpenSSH 默认私钥文件，返回首个存在的 `~/.ssh/` 路径。
fn default_identity_file() -> Option<String> {
    let home = dirs::home_dir()?;
    ["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"]
        .into_iter()
        .find(|candidate| home.join(".ssh").join(candidate).is_file())
        .map(|candidate| format!("~/.ssh/{candidate}"))
}

/// 把任意别名收敛为合法 server id（小写字母/数字/-/_），非法字符折叠为单个 `-`。
fn sanitize_server_id(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
            out.push(ch);
        } else if ch.is_ascii_uppercase() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    let truncated: String = out.chars().take(ID_MAX_LEN).collect();
    if truncated.is_empty() {
        "server".to_string()
    } else {
        truncated
    }
}

fn unique_server_id(base: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 2;
    loop {
        // 给 `-后缀` 预留空间：传入的 base 可能已是 ID_MAX_LEN 上限
        // （sanitize_server_id 的截断结果），直接追加后缀会超长，
        // 保存时被 validate_server_id 拒绝。
        let suffix_text = suffix.to_string();
        let truncated: String = base
            .chars()
            .take(ID_MAX_LEN.saturating_sub(suffix_text.len() + 1))
            .collect();
        let candidate = format!("{truncated}-{suffix_text}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_tool::validation::validate_server_id;

    #[test]
    fn parse_ssh_config_extracts_host_blocks() {
        let content = r#"
# 注释行
Host prod-web
    HostName 10.0.1.12
    User ops
    Port 2222
    IdentityFile ~/.ssh/prod_key

Host "quoted-alias" extra.pattern.*
    HostName=example.com
    User=root

Host *
    User defaultuser

Host 中文服务器
    HostName 192.168.1.2
"#;
        let servers = parse_ssh_config(content);
        assert_eq!(servers.len(), 3);

        let first = &servers[0];
        assert_eq!(first.id, "prod-web");
        assert_eq!(first.name, "prod-web");
        assert_eq!(first.host, "10.0.1.12");
        assert_eq!(first.port, 2222);
        assert_eq!(first.username, "ops");
        assert_eq!(first.auth_method, SshAuthMethod::Key);
        assert_eq!(first.private_key_path, "~/.ssh/prod_key");

        // `Key=value` 写法 + 引号别名 + 多模式行取第一个具体别名
        let second = &servers[1];
        assert_eq!(second.name, "quoted-alias");
        assert_eq!(second.host, "example.com");
        assert_eq!(second.username, "root");
        assert_eq!(second.port, 22);

        // 中文别名：name 原样保留，id 收敛为合法字符；通配块 User 作为默认值
        let third = &servers[2];
        assert_eq!(third.name, "中文服务器");
        assert_eq!(third.id, "server");
        assert_eq!(third.host, "192.168.1.2");
        assert_eq!(third.username, "defaultuser");
    }

    #[test]
    fn parse_ssh_config_handles_quoted_alias_with_spaces_and_multi_identity_files() {
        let content = r#"
Host "my host"
    HostName 10.1.1.1
    IdentityFile ~/.ssh/first_key ~/.ssh/second_key
"#;
        let servers = parse_ssh_config(content);
        assert_eq!(servers.len(), 1);
        // 带空格的引号别名完整保留，不再被空白切断。
        assert_eq!(servers[0].name, "my host");
        assert_eq!(servers[0].id, "my-host");
        // 一行多个 IdentityFile 取第一个，而不是把整行当成一个路径。
        assert_eq!(servers[0].private_key_path, "~/.ssh/first_key");
    }

    #[test]
    fn parse_ssh_config_dedupes_ids() {
        let content = "Host web\nHostName a.example\nHost web\nHostName b.example\n";
        let servers = parse_ssh_config(content);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].id, "web");
        assert_eq!(servers[1].id, "web-2");
    }

    #[test]
    fn parse_ssh_config_merges_defaults_across_multiple_host_star_blocks() {
        // OpenSSH 逐关键字「先命中优先」：第二个 `Host *` 块补上第一个缺失的关键字。
        let content = r#"
Host db
    HostName 10.0.0.5

Host *
    Port 2222

Host *
    User deploy
    IdentityFile ~/.ssh/shared_key
"#;
        let servers = parse_ssh_config(content);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].port, 2222);
        assert_eq!(servers[0].username, "deploy");
        assert_eq!(servers[0].private_key_path, "~/.ssh/shared_key");
    }

    #[test]
    fn parse_ssh_config_pattern_wildcard_block_is_not_global_default() {
        // `Host *.example.com` 只匹配相应模式的主机，不能作为全局默认值来源。
        let content = r#"
Host *.example.com
    User pattern-user
    Port 2020

Host web
    HostName web.internal
"#;
        let servers = parse_ssh_config(content);
        assert_eq!(servers.len(), 1);
        assert_ne!(servers[0].username, "pattern-user");
        assert_eq!(servers[0].port, 22);
    }

    #[test]
    fn unique_server_id_keeps_candidates_within_length_limit() {
        let mut used = std::collections::HashSet::new();
        let max_base = "a".repeat(ID_MAX_LEN);
        assert_eq!(unique_server_id(&max_base, &mut used), max_base);
        // 已满长的 base 去重后追加后缀，仍须通过保存侧校验。
        let second = unique_server_id(&max_base, &mut used);
        assert!(
            second.chars().count() <= ID_MAX_LEN,
            "去重 id 超长：{second}"
        );
        assert!(
            validate_server_id(&second).is_ok(),
            "去重 id 非法：{second}"
        );
    }

    #[test]
    fn sanitize_server_id_normalizes() {
        assert_eq!(sanitize_server_id("Prod-Web_1"), "prod-web_1");
        assert_eq!(sanitize_server_id("我的 服务器"), "server");
        assert_eq!(sanitize_server_id("--odd..name--"), "odd-name");
    }
}
