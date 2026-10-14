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
    const unlistenPromise = listen<ToolConfirmRequest>("tool-confirm-request", (event) => {
      enqueueToolConfirm(event.payload);
    });
    unlistenPromise.catch((error) => {
      console.error("工具确认监听注册失败:", error);
    });
    return () => {
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch(() => {
          // 注册失败的清理：无需退订，也无需再报错（上面已记录）。
        });
    };
  }, []);

  const pending = useSyncExternalStore(subscribeToolConfirms, getPendingToolConfirms);
  const current = pending[0];
  if (!current) return null;

  const resolve = async (approved: boolean) => {
    try {
      const consumed = await invoke<boolean>("tool_confirm_resolve", {
        workspaceId: current.workspaceId,
        requestId: current.requestId,
        approved,
      });
      // 回传成功才出队：失败（命令未注册/序列化失败等）时保留条目，用户可
      // 重试，避免静默关闭审查门禁只能等后端 180s 超时。
      removeToolConfirm(current.requestId);
      // 未消费（后端已超时/取消清槽）：提示用户该确认已失效，避免以为已生效。
      if (!consumed) {
        toast.warning("该确认请求已失效（可能已超时或任务已取消）。");
      }
    } catch (error) {
      // 回传持续失败时按 fail-closed 出队：本弹窗是受控模态，出队是唯一关闭
      // 途径，失败保留会让 UI 永久卡死且无放弃出口；后端侧等待会按超时拒绝
      // 收口（命令不会执行），关闭弹窗不等于放行。
      removeToolConfirm(current.requestId);
      toast.error(
        `提交确认失败（${String(error)}），本次确认未生效；后端将按超时拒绝处理，命令不会执行。`,
      );
    }
  };

  return (
    <ToolConfirmDialog
      open
      request={current}
      remaining={pending.length - 1}
      onApprove={() => void resolve(true)}
      onReject={() => void resolve(false)}
    />
  );
}
