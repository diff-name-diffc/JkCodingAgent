//! 系统剪贴板的「文件」读写。
//!
//! 用途：文件树里选中条目后 ⌘C，再在 Finder / 其他应用里 ⌘V 粘贴出真实文件；
//! 反向也能读回 Finder 复制的文件列表，交给 `workspace::fs::copy_fs_entry`
//! 拷进项目树。
//!
//! macOS 侧走 `NSPasteboard`：
//! - 写：`writeObjects` 传入 `NSURL` 数组，AppKit 会自动带上
//!   `public.file-url` + `NSFilenamesPboardType` 两类 flavor（实测 Finder 因此
//!   能粘贴出文件）；需要「粘贴成路径文本」时再补一条
//!   `public.utf8-plain-text`（文本编辑器/终端粘贴得到路径）。
//! - 读：优先 `NSFilenamesPboardType` 的 propertyList（Finder 多选时只有它
//!   带全集），为空回退 `public.file-url` 单条。
//!
//! 非 macOS 平台只保证可编译：写返回「暂不支持」，读返回空数组。
//!
//! 真实剪贴板操作不做端到端单测（会污染系统剪贴板、并触发 macOS 的剪贴板
//! 访问提示）；macOS 用例改用同名私有 pasteboard 走完全相同的 AppKit 代码路径。

use anyhow::Context;

use crate::shared::error::{CommandResult, IntoCommandResult};

/// 剪贴板文件读写错误（`Display` 文案直接暴露给用户）。
#[derive(Debug, thiserror::Error)]
pub enum ClipboardFilesError {
    /// paths 与 text 都为空：没有任何可写入的内容。
    #[error("没有可复制的内容")]
    EmptyPayload,
    /// `NSPasteboard` 拒绝写入（writeObjects / setString 返回 false）。
    #[error("写入系统剪贴板失败")]
    WriteFailed,
    /// 非 macOS 平台的显式拒绝（保证跨平台可编译）；macOS 构建下不会被构造。
    #[allow(dead_code)]
    #[error("当前平台暂不支持复制文件到系统剪贴板")]
    UnsupportedPlatform,
}

/// 归一化写入载荷：丢掉空路径、按首次出现去重（同一路径重复进剪贴板没有
/// 意义，还会让 Finder 粘贴出多份），空文本折叠为 `None`。
///
/// 两者皆空时返回 [`ClipboardFilesError::EmptyPayload`]。
fn resolve_payload(
    paths: &[String],
    text: Option<&str>,
) -> Result<(Vec<String>, Option<String>), ClipboardFilesError> {
    let mut normalized: Vec<String> = Vec::with_capacity(paths.len());
    for path in paths {
        if path.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == path) {
            normalized.push(path.clone());
        }
    }

    // 全空白视作「没给文本」；非空文本原样保留（路径两侧空格是合法字符）。
    let text = text
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    if normalized.is_empty() && text.is_none() {
        return Err(ClipboardFilesError::EmptyPayload);
    }

    Ok((normalized, text))
}

/// 把文件路径列表写入系统剪贴板（可选附一条路径文本）。
// 同步命令：Tauri 中非 async 命令在主线程执行，与 NSPasteboard 的主线程惯例
// 一致；pasteboard 读写开销微小，不构成主线程负担。
#[tauri::command]
pub fn clipboard_write_files(paths: Vec<String>, text: Option<String>) -> CommandResult<()> {
    clipboard_write_files_impl(paths, text)
        .with_context(|| "复制文件路径到系统剪贴板失败")
        .into_command_result()
}

fn clipboard_write_files_impl(
    paths: Vec<String>,
    text: Option<String>,
) -> Result<(), ClipboardFilesError> {
    let (paths, text) = resolve_payload(&paths, text.as_deref())?;
    write_payload(&paths, text.as_deref())
}

/// 读取系统剪贴板中的文件路径；没有文件（含只有文本）返回空数组。
#[tauri::command]
pub fn clipboard_read_files() -> CommandResult<Vec<String>> {
    read_payload()
        .with_context(|| "读取系统剪贴板文件失败")
        .into_command_result()
}

#[cfg(target_os = "macos")]
fn write_payload(paths: &[String], text: Option<&str>) -> Result<(), ClipboardFilesError> {
    use objc2_app_kit::NSPasteboard;

    let pasteboard = NSPasteboard::generalPasteboard();
    write_payload_on(&pasteboard, paths, text)
}

/// 往指定 pasteboard 写入（真实路径走 generalPasteboard，用例走私有同名板）。
#[cfg(target_os = "macos")]
fn write_payload_on(
    pasteboard: &objc2_app_kit::NSPasteboard,
    paths: &[String],
    text: Option<&str>,
) -> Result<(), ClipboardFilesError> {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSPasteboardWriting;
    use objc2_foundation::{NSArray, NSString, NSURL};

    autoreleasepool(|_pool| {
        pasteboard.clearContents();

        if !paths.is_empty() {
            // NSURL 的 writableTypesForPasteboard 会给出 public.file-url 与
            // NSFilenamesPboardType：Finder / 其他应用因此能粘贴出真实文件。
            let objects: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
                .iter()
                .map(|path| {
                    let path = NSString::from_str(path);
                    let url = NSURL::fileURLWithPath(&path);
                    ProtocolObject::from_retained(url)
                })
                .collect();
            let array = NSArray::from_retained_slice(&objects);
            if !pasteboard.writeObjects(&array) {
                return Err(ClipboardFilesError::WriteFailed);
            }
        }

        if let Some(text) = text {
            // 追加文本 flavor：文本类应用粘贴得到路径，Finder 仍按文件处理。
            let text = NSString::from_str(text);
            if !pasteboard.setString_forType(&text, pasteboard_type_string()) {
                return Err(ClipboardFilesError::WriteFailed);
            }
        }

        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
fn write_payload(_paths: &[String], _text: Option<&str>) -> Result<(), ClipboardFilesError> {
    Err(ClipboardFilesError::UnsupportedPlatform)
}

#[cfg(target_os = "macos")]
fn read_payload() -> Result<Vec<String>, ClipboardFilesError> {
    use objc2_app_kit::NSPasteboard;

    let pasteboard = NSPasteboard::generalPasteboard();
    read_payload_on(&pasteboard)
}

/// 从指定 pasteboard 读取文件路径（语义见 [`read_payload`]）。
#[cfg(target_os = "macos")]
fn read_payload_on(
    pasteboard: &objc2_app_kit::NSPasteboard,
) -> Result<Vec<String>, ClipboardFilesError> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSPasteboardTypeFileURL;
    use objc2_foundation::{NSArray, NSString};

    autoreleasepool(|_pool| {
        // ① 传统文件名数组 flavor：Finder 多选复制时只有它携带完整列表。
        // bindings 里该常量标了 deprecated（10.14 起推荐 public.file-url），
        // 但老应用仍只认它，读取侧保留。
        #[allow(deprecated)]
        let filenames: Vec<String> = pasteboard
            .propertyListForType(unsafe { objc2_app_kit::NSFilenamesPboardType })
            .and_then(|plist| plist.downcast::<NSArray>().ok())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|item| item.downcast_ref::<NSString>().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if !filenames.is_empty() {
            return Ok(filenames);
        }

        // ② 回退 public.file-url：单条 file:// URL，交给 NSURL 解析
        //（percent 编码、中文与空格都能还原，比手写解码可靠）。
        if let Some(raw) = pasteboard.stringForType(unsafe { NSPasteboardTypeFileURL }) {
            if let Some(path) = file_url_to_path(&raw.to_string()) {
                return Ok(vec![path]);
            }
        }

        // 剪贴板只有文本/其他内容：不是错误，返回空集合。
        Ok(Vec::new())
    })
}

#[cfg(not(target_os = "macos"))]
fn read_payload() -> Result<Vec<String>, ClipboardFilesError> {
    Ok(Vec::new())
}

/// `NSPasteboardTypeString` 是 extern 静态符号，读取必须 unsafe。
#[cfg(target_os = "macos")]
fn pasteboard_type_string() -> &'static objc2_app_kit::NSPasteboardType {
    unsafe { objc2_app_kit::NSPasteboardTypeString }
}

/// `file://` URL → 本地绝对路径（percent 解码由 NSURL 完成）。
#[cfg(target_os = "macos")]
fn file_url_to_path(raw: &str) -> Option<String> {
    use objc2_foundation::{NSString, NSURL};

    let raw = NSString::from_str(raw);
    let url = NSURL::URLWithString(&raw)?;
    url.path().map(|path| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{resolve_payload, ClipboardFilesError};

    #[test]
    fn resolve_payload_dedupes_paths_and_keeps_order() {
        let paths = vec![
            "/a/b.txt".to_string(),
            "/a/b.txt".to_string(),
            String::new(),
            "/a/c.txt".to_string(),
        ];
        let (normalized, text) = resolve_payload(&paths, Some("/a/b.txt")).unwrap();
        assert_eq!(normalized, vec!["/a/b.txt", "/a/c.txt"]);
        assert_eq!(text.as_deref(), Some("/a/b.txt"));
    }

    #[test]
    fn resolve_payload_folds_blank_text_to_none() {
        // 有路径时空白文本折叠为 None：只写文件 flavor，不额外塞一条空字符串。
        let (normalized, text) = resolve_payload(&["/a/b.txt".to_string()], Some("   ")).unwrap();
        assert_eq!(normalized, vec!["/a/b.txt"]);
        assert!(text.is_none());
    }

    #[test]
    fn resolve_payload_rejects_empty_payload() {
        assert!(matches!(
            resolve_payload(&[], None),
            Err(ClipboardFilesError::EmptyPayload)
        ));
        assert!(matches!(
            resolve_payload(&[String::new()], Some("")),
            Err(ClipboardFilesError::EmptyPayload)
        ));
    }
}

/// macOS 行为用例：用私有同名 pasteboard 走与生产完全相同的 AppKit 代码路径，
/// 不动系统剪贴板（避免污染用户内容与 macOS 剪贴板访问提示）。
#[cfg(all(test, target_os = "macos"))]
mod macos_pasteboard_tests {
    use super::{read_payload_on, write_payload_on};
    use crate::test_util::TempDirGuard;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    fn private_pasteboard(tag: &str) -> objc2::rc::Retained<NSPasteboard> {
        let name = NSString::from_str(&format!("jkcodingagent-clipboard-test-{tag}"));
        NSPasteboard::pasteboardWithName(&name)
    }

    fn pasteboard_types(pasteboard: &NSPasteboard) -> Vec<String> {
        pasteboard
            .types()
            .map(|types| types.iter().map(|item| item.to_string()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn writes_file_url_flavors_and_reads_back_the_path() {
        let dir = TempDirGuard::new("aha-clipboard");
        let file = dir.path().join("示例 file.txt");
        std::fs::write(&file, "hi").unwrap();
        let path = file.to_string_lossy().into_owned();

        let pasteboard = private_pasteboard("write");
        write_payload_on(&pasteboard, std::slice::from_ref(&path), Some("文本侧")).unwrap();

        // Finder 只认这两类 flavor：缺了它们「粘到访达」就退化成粘一段文本。
        let types = pasteboard_types(&pasteboard);
        assert!(
            types.iter().any(|item| item == "public.file-url"),
            "缺少 public.file-url：{types:?}"
        );
        assert!(
            types.iter().any(|item| item == "NSFilenamesPboardType"),
            "缺少 NSFilenamesPboardType：{types:?}"
        );

        // 中文字符与空格经 NSURL fileURLWithPath 往返不应丢字符。
        assert_eq!(read_payload_on(&pasteboard).unwrap(), vec![path]);
    }

    #[test]
    fn reads_nothing_when_pasteboard_only_holds_text() {
        let pasteboard = private_pasteboard("text-only");
        pasteboard.clearContents();
        let text = NSString::from_str("just some text");
        assert!(pasteboard.setString_forType(&text, super::pasteboard_type_string()));

        assert!(read_payload_on(&pasteboard).unwrap().is_empty());
    }
}
