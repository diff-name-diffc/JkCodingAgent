import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { McpStatus } from "../types";

/** 聊天头部/项目头部共用的 MCP 状态指示灯映射。 */
export function getMcpIndicatorState(
  mcpStatus: McpStatus | null,
  mcpChecking: boolean,
): { color: string; label: string } {
  if (mcpChecking) {
    return { color: "var(--warning)", label: "检查中" };
  }
  if (!mcpStatus || mcpStatus.aggregate === "not_configured") {
    return { color: "var(--text-hint)", label: "未配置" };
  }
  if (mcpStatus.aggregate === "healthy") {
    return { color: "var(--success)", label: "正常" };
  }
  return { color: "var(--danger)", label: "异常" };
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
      const nextStatus = await invoke<McpStatus>("mcp_project_status", { projectPath });
      setStatus(nextStatus);
    } catch (error) {
      console.error("mcp_project_status 失败:", error);
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
      const nextStatus = await invoke<McpStatus>("mcp_global_status");
      setStatus(nextStatus);
    } catch (error) {
      console.error("mcp_global_status 失败:", error);
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
