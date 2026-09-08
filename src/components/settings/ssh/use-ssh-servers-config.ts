import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SshAuditLog, SshToolsConfig } from "../../../types";
import { publishSaveSource, registerSaveSource } from "../save-sources";
import { toast } from "../toast";

const AUTOSAVE_DELAY_MS = 400;
const SAVE_SOURCE_ID = "ssh-servers";

/**
 * SSH 服务器配置的加载与 400ms debounce 自动保存管线（UI-21 遗留领取时自
 * SshServersPage 抽出——页面 481 行逼近 500 行规模红线，新增统一保存状态
 * 接线前按职责先拆分）。
 *
 * 保存状态同步发布到 save-sources 注册表（`ssh-servers` 源）：头部指示器与
 * 关闭弹窗的脏检查聚合消费。卸载（切导航页）时 flush-then-clear——修复
 * 400ms debounce 窗口内切页丢编辑的既有缺陷（此前 cleanup 只 clearTimeout）。
 */
export function useSshServersConfig() {
  const [config, setConfig] = useState<SshToolsConfig>({ servers: [] });
  const [audit, setAudit] = useState<SshAuditLog>({ records: [] });
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<{ fieldId?: string; message: string } | null>(null);

  // 自动保存是异步的，通过 ref 读取最新状态，避免闭包捕获过期值。
  const configRef = useRef(config);
  configRef.current = config;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fieldIdRef = useRef<string | undefined>(undefined);
  const savingRef = useRef(false);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const snapshot = await invoke<{ servers: SshToolsConfig["servers"]; audit: SshAuditLog }>(
        "ssh_tool_load_settings",
      );
      setConfig({ servers: snapshot.servers });
      setAudit(snapshot.audit);
    } catch (err) {
      setLoadError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const saveNow = useCallback(async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    publishSaveSource(SAVE_SOURCE_ID, {
      mode: "auto",
      dirty: false,
      saving: true,
      hasError: false,
    });
    try {
      const savedConfig = await invoke<SshToolsConfig>("ssh_tool_save_config", {
        config: configRef.current,
      });
      setConfig(savedConfig);
      setSaveError(null);
      publishSaveSource(SAVE_SOURCE_ID, {
        mode: "auto",
        dirty: false,
        saving: false,
        hasError: false,
      });
      toast.success("已保存");
    } catch (err) {
      const message = String(err);
      setSaveError({ fieldId: fieldIdRef.current, message });
      publishSaveSource(SAVE_SOURCE_ID, {
        mode: "auto",
        dirty: false,
        saving: false,
        hasError: true,
      });
      toast.error(`保存失败：${message}`);
    } finally {
      savingRef.current = false;
    }
  }, []);

  const scheduleSave = useCallback(
    (fieldId?: string) => {
      fieldIdRef.current = fieldId ?? fieldIdRef.current;
      setSaveError(null);
      if (timerRef.current) clearTimeout(timerRef.current);
      publishSaveSource(SAVE_SOURCE_ID, {
        mode: "auto",
        dirty: true,
        saving: false,
        hasError: false,
      });
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void saveNow();
      }, AUTOSAVE_DELAY_MS);
    },
    [saveNow],
  );

  useEffect(() => {
    loadConfig();
    const unregister = registerSaveSource(SAVE_SOURCE_ID, async () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      await saveNow();
    });
    return () => {
      unregister();
      // flush-then-clear：卸载前把 debounce 窗口内的待保存编辑落盘——saveNow 经
      // configRef 读最新值，组件销毁后的 setState 为 no-op，不影响落库；保存
      // 仍在进行时（savingRef）saveNow 直接返回，在途保存同样覆盖最新值。
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
        void saveNow();
      }
    };
  }, [loadConfig, saveNow]);

  return {
    config,
    setConfig,
    audit,
    loading,
    loadError,
    saveError,
    scheduleSave,
    loadConfig,
    saveNow,
  };
}
