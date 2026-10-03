import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "../Toast";
import { publishSaveSource, registerSaveSource } from "./save-sources";

/** 设置中心自动保存的统一 debounce 间隔（use-aha-settings 单例管线同用此值）。 */
export const AUTOSAVE_DELAY_MS = 400;

export interface AutoSaveSourceError {
  /** 触发本次保存的编辑点标识（SSH 卡片按字段内联报错用）；未提供时缺省。 */
  fieldId?: string;
  message: string;
}

interface AutoSaveSourceOptions<T> {
  /** save-sources 注册表中的源 id（头部指示器与关闭脏检查聚合消费）。 */
  id: string;
  /** 读取待保存的最新值（hook 经 ref 转发，避免闭包捕获过期状态）。 */
  read: () => T;
  /** 落盘；抛错视为保存失败（hook 统一 toast + 发布 error 状态）。 */
  save: (value: T) => Promise<void>;
}

/**
 * 设置页级「400ms debounce 自动保存」统一管线：SSH 服务器页与 MCP 页此前各自
 * 内联同一套 timer/saving 守卫/save-sources 发布/卸载 flush 逻辑，语义与文案
 * 随实现漂移——收敛到本 hook 单一出处。
 *
 * 语义（与收敛前两处实现逐项对齐）：
 * - 保存成功不弹 toast：头部 SaveStatusIndicator 持续显示「已保存」；
 * - 失败统一 `toast.error("保存失败：…")` + `error` 状态（含 fieldId，供字段
 *   内联报错）；下一次 scheduleSave 清除错误；
 * - 并发守卫：保存进行中再触发只跳过（在途保存读取的是 read() 最新值）；
 * - 卸载 flush-then-clear：仅 debounce timer 活跃时落盘，修复切导航页丢
 *   400ms 窗口内编辑的缺陷；组件销毁后的 setState 由 React 视为 no-op。
 */
export function useAutoSaveSource<T>({ id, read, save }: AutoSaveSourceOptions<T>) {
  const [error, setError] = useState<AutoSaveSourceError | null>(null);

  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);
  const fieldIdRef = useRef<string | undefined>(undefined);
  const readRef = useRef(read);
  readRef.current = read;
  const saveRef = useRef(save);
  saveRef.current = save;

  const saveNow = useCallback(async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    publishSaveSource(id, { mode: "auto", dirty: false, saving: true, hasError: false });
    try {
      await saveRef.current(readRef.current());
      setError(null);
      publishSaveSource(id, { mode: "auto", dirty: false, saving: false, hasError: false });
    } catch (err) {
      const message = String(err);
      setError({ fieldId: fieldIdRef.current, message });
      publishSaveSource(id, { mode: "auto", dirty: false, saving: false, hasError: true });
      toast.error(`保存失败：${message}`);
    } finally {
      savingRef.current = false;
    }
  }, [id]);

  const scheduleSave = useCallback(
    (fieldId?: string) => {
      fieldIdRef.current = fieldId ?? fieldIdRef.current;
      setError(null);
      if (timerRef.current) clearTimeout(timerRef.current);
      publishSaveSource(id, { mode: "auto", dirty: true, saving: false, hasError: false });
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void saveNow();
      }, AUTOSAVE_DELAY_MS);
    },
    [id, saveNow],
  );

  /** 立即落盘：清 debounce timer 后保存（注册表 flush / 模式切换共用）。 */
  const flush = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    return saveNow();
  }, [saveNow]);

  /** 丢弃等待中的保存（不落盘）：编辑内容回到无效态（如 JSON 解析失败）时用。 */
  const cancelPending = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  useEffect(() => {
    const unregister = registerSaveSource(id, flush);
    return () => {
      unregister();
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
        void saveNow();
      }
    };
  }, [id, flush, saveNow]);

  return { scheduleSave, saveNow, flush, cancelPending, error };
}
