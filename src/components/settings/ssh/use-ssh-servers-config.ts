import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SshAuditLog, SshToolsConfig } from "../../../types";
import { useAutoSaveSource } from "../use-auto-save-source";

const SAVE_SOURCE_ID = "ssh-servers";

/**
 * SSH 服务器配置的加载与自动保存管线（保存段基于共享 useAutoSaveSource）。
 *
 * 保存状态同步发布到 save-sources 注册表（`ssh-servers` 源）：头部指示器与
 * 关闭弹窗的脏检查聚合消费。卸载（切导航页）时 flush-then-clear——修复
 * 400ms debounce 窗口内切页丢编辑的既有缺陷。
 */
export function useSshServersConfig() {
  const [config, setConfig] = useState<SshToolsConfig>({ servers: [] });
  const [audit, setAudit] = useState<SshAuditLog>({ records: [] });
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

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

  const { scheduleSave, saveNow, flush, error: saveError } = useAutoSaveSource({
    id: SAVE_SOURCE_ID,
    // hook 经 ref 持有最新渲染的 read 闭包，保存时读到的即最新 entries/config。
    read: () => config,
    save: async (next) => {
      // 后端返回规范化后的配置（id 补全等）；组件销毁后的 setState 为 no-op。
      const savedConfig = await invoke<SshToolsConfig>("ssh_tool_save_config", { config: next });
      setConfig(savedConfig);
    },
  });

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
    flush,
  };
}
