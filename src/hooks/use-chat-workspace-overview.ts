import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type { ChatWorkspaceOverview } from "../types";

/**
 * 「分类文件」面板的目录总览（chat_workspace_overview）：分类共享工程目录
 * 逐条校验 + 指定会话的临时目录路径。切换会话即换 key 重新拉取。
 */
export function useChatWorkspaceOverviewQuery(
  categoryId: string,
  sessionId: string | null,
  enabled = true,
) {
  return useQuery({
    queryKey: ["chat", "workspace-overview", categoryId, sessionId],
    queryFn: () =>
      invoke<ChatWorkspaceOverview>("chat_workspace_overview", {
        categoryId,
        sessionId,
      }),
    enabled,
  });
}
