/**
 * 命令审查「需用户确认」弹窗的全局宿主：注册后端事件监听 + 渲染待确认对话框。
 *
 * 后端命令类工具审查不通过且需人工放行时 emit `tool-confirm-request`；本组件
 * 入队到 store，`ToolConfirmDialog` 逐个展示，用户裁决经
 * `invoke("tool_confirm_resolve")` 回传解除后端等待。挂在应用根部（App.tsx），
 * 与 ToastProvider 同级，保证任何视图（含画布/工作流）都能收到确认请求。
 */

import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "../Toast";
import {
  enqueueToolConfirm,
  getPendingToolConfirms,
  removeToolConfirm,
  subscribeToolConfirms,
  type ToolConfirmRequest,
} from "./tool-confirm-store";
import { ToolConfirmDialog } from "./ToolConfirmDialog";

export function ToolConfirmHost() {
  useEffect(() => {
    let disposed = false;
    const unlistenPromise = listen<ToolConfirmRequest>("tool-confirm-request", (event) => {
      enqueueToolConfirm(event.payload);
    });
    return () => {
      disposed = true;
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          if (!disposed) console.error("工具确认监听注册失败:", error);
        });
    };
  }, []);

  const pending = useSyncExternalStore(subscribeToolConfirms, getPendingToolConfirms);
  const current = pending[0];
  if (!current) return null;

  const resolve = async (approved: boolean) => {
    removeToolConfirm(current.requestId);
    try {
      const consumed = await invoke<boolean>("tool_confirm_resolve", {
        workspaceId: current.workspaceId,
        requestId: current.requestId,
        approved,
      });
      // 未消费（后端已超时/取消清槽）：提示用户该确认已失效，避免以为已生效。
      if (!consumed) {
        toast.warning("该确认请求已失效（可能已超时或任务已取消）。");
      }
    } catch (error) {
      toast.error(`提交确认失败：${String(error)}`);
    }
  };

  return (
    <ToolConfirmDialog
      open
      request={current}
      remaining={pending.length}
      onApprove={() => void resolve(true)}
      onReject={() => void resolve(false)}
    />
  );
}
