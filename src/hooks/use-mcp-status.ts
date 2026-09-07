import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { McpStatus } from "../types";

/**
 * MCP 连接聚合状态键（UI-22b）：映射到 status-meta 的 "connection" 域，由
 * StatusPill 统一渲染图标 + 文字 + 配色，取代旧「内联硬编码色点 + 压平异常」。
 * 保留后端 aggregate 的 degraded / invalid_config 区分（不再都叫「异常」）；
 * 真实失败服务器与原因在 MCP 状态弹窗内按服务器展开。
 */
export function getMcpConnectionStatus(
  mcpStatus: McpStatus | null,
  mcpChecking: boolean,
): string {
  if (mcpChecking) return "checking";
  if (!mcpStatus) return "not_configured";
  return mcpStatus.aggregate;
}

/**
 * 项目作用域 MCP 状态：`enabled` 为真（页面可见）时刷新，开关服务器走
 * `mcp_project_set_server_enabled`（全局条目 copy-on-write 进项目文件）。
 */
export function useProjectMcpStatus(projectPath: string, enabled: boolean) {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [updatingServer, setUpdatingServer] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setChecking(true);
    try {
      const nextStatus = await invoke<McpStatus>("mcp_status", { projectPath });
      setStatus(nextStatus);
    } catch (error) {
      console.error("mcp_status（项目作用域）失败:", error);
    } finally {
      setChecking(false);
    }
  }, [projectPath]);

  const setServerEnabled = useCallback(
    async (serverName: string, enabled: boolean) => {
      setUpdatingServer(serverName);
      try {
        const nextStatus = await invoke<McpStatus>("mcp_project_set_server_enabled", {
          projectPath,
          serverName,
          enabled,
        });
        setStatus(nextStatus);
      } catch (error) {
        console.error("mcp_project_set_server_enabled 失败:", error);
      } finally {
        setUpdatingServer(null);
      }
    },
    [projectPath],
  );

  useEffect(() => {
    if (enabled) {
      void refresh();
    }
  }, [enabled, refresh]);

  return { status, checking, updatingServer, refresh, setServerEnabled };
}

/**
 * 全局作用域 MCP 状态：所有聊天会话共享。`enabled` 控制是否取数
 * （聊天页挂载时才刷新）。配置编辑在设置中心「MCP 服务器」页。
 */
export function useGlobalMcpStatus(enabled: boolean) {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [checking, setChecking] = useState(false);

  const refresh = useCallback(async () => {
    setChecking(true);
    try {
      // 聊天页头部指示灯需要真实探活：强制全量刷新，不复用新鲜窗口缓存。
      const nextStatus = await invoke<McpStatus>("mcp_status", {
        projectPath: null,
        forceRefresh: true,
      });
      setStatus(nextStatus);
    } catch (error) {
      console.error("mcp_status（全局作用域）失败:", error);
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (enabled) {
      void refresh();
    }
  }, [enabled, refresh]);

  return { status, checking, refresh };
}
