/**
 * 文件树的剪贴板动作接线：⌘C 把选中项写成系统剪贴板里的「真实文件」，
 * ⌘V 把剪贴板里的文件复制进当前落点目录（可来自本应用，也可来自访达）。
 *
 * 命令契约见 src-tauri：`clipboard_write_files` / `clipboard_read_files` /
 * `copy_fs_entry`。落点与文案的判定是纯函数（file-clipboard-actions.ts）。
 */

import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "../Toast";
import { readFilesFromClipboard, writeFilesToClipboard } from "../../lib/file-clipboard";
import {
  describeCopiedEntries,
  describePastedEntries,
  resolvePasteDirectory,
  type ClipboardNodeRef,
} from "./file-clipboard-actions";

function resolveErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function useFileTreeClipboard({
  projectPath,
  onRefresh,
}: {
  projectPath: string;
  /** 粘贴后刷新树，让新落盘的条目立即可见。 */
  onRefresh: () => Promise<void> | void;
}) {
  const [busy, setBusy] = useState(false);
  // 并发守卫必须同步生效：同一 tick 内的两次触发（菜单 onSelect 与快捷键）
  // 都会读到提交前的旧 state，会并发执行两次 copy_fs_entry 落出多余副本。
  const pastingRef = useRef(false);

  const copyEntry = useCallback(async (node: ClipboardNodeRef | null) => {
    if (!node) {
      toast.error("请先选中要复制的文件");
      return;
    }

    try {
      await writeFilesToClipboard([node.path]);
      toast.success(describeCopiedEntries([node]));
    } catch (error) {
      console.error("复制文件到剪贴板失败:", error);
      toast.error(`复制失败：${resolveErrorMessage(error)}`);
    }
  }, []);

  const pasteInto = useCallback(
    async (target: ClipboardNodeRef | null) => {
      if (pastingRef.current) {
        return;
      }

      pastingRef.current = true;
      setBusy(true);
      try {
        const sources = await readFilesFromClipboard();
        if (sources.length === 0) {
          toast.error("剪贴板里没有可粘贴的文件；先在文件列表或访达中复制");
          return;
        }

        const created = await invoke<string[]>("copy_fs_entry", {
          sourcePaths: sources,
          destinationDir: resolvePasteDirectory(target, projectPath),
          projectPath,
        });
        toast.success(describePastedEntries(created));
        await onRefresh();
      } catch (error) {
        console.error("粘贴剪贴板文件失败:", error);
        toast.error(`粘贴失败：${resolveErrorMessage(error)}`);
      } finally {
        pastingRef.current = false;
        setBusy(false);
      }
    },
    [onRefresh, projectPath],
  );

  return { busy, copyEntry, pasteInto };
}
