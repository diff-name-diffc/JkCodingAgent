/**
 * 文件级系统剪贴板（复制真实文件，而非路径文本）。
 *
 * 为什么不能走 navigator.clipboard / Tauri clipboard 插件：两者只支持文本与
 * 图片，写不进 `public.file-url` / `NSFilenamesPboardType` 两类 flavor，粘到
 * 访达里只会得到一段路径文本。后端 `platform::clipboard_files` 直接用
 * NSPasteboard 写入文件 URL，同时附带路径文本——因此在访达 / 其他应用里粘贴
 * 得到文件本体，在文本编辑器里粘贴得到路径，两条诉求一次满足。
 *
 * 读取侧供 ⌘V 使用：把剪贴板里的文件列表（可能来自本应用，也可能来自访达）
 * 交给 `copy_fs_entry` 落到目标目录。
 */

import { invoke } from "@tauri-apps/api/core";

function normalizePaths(paths: readonly string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const path of paths) {
    // 只把全空白串当作「空」跳过：POSIX 文件名允许首尾空格（后端
    // resolve_payload 也是原样保留），trim 后会对不上真实文件。
    if (!path.trim() || seen.has(path)) {
      continue;
    }
    seen.add(path);
    result.push(path);
  }
  return result;
}

/**
 * 把文件系统条目写入系统剪贴板。
 * 文本侧写入换行拼接的绝对路径，供纯文本应用使用；文件侧写入文件 URL。
 */
export async function writeFilesToClipboard(paths: readonly string[]): Promise<void> {
  const normalized = normalizePaths(paths);
  if (normalized.length === 0) {
    throw new Error("没有可复制的文件");
  }

  await invoke<void>("clipboard_write_files", {
    paths: normalized,
    text: normalized.join("\n"),
  });
}

/** 读取系统剪贴板里的文件绝对路径；剪贴板中无文件时返回空数组。 */
export async function readFilesFromClipboard(): Promise<string[]> {
  const paths = await invoke<string[]>("clipboard_read_files");
  return Array.isArray(paths) ? normalizePaths(paths) : [];
}

export const __testing = { normalizePaths };
