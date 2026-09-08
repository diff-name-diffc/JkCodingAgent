import { useSyncExternalStore } from "react";
import { Check, Loader2, PencilLine, RotateCw } from "lucide-react";
import { useAhaSettings } from "./use-aha-settings";
import { aggregateSaveStatuses } from "./save-status";
import { flushAllSaveSources, getSaveSourcesSnapshot, subscribeSaveSources } from "./save-sources";

/**
 * 设置内容头部的持续保存状态指示器（UI-21；遗留领取扩展为多源聚合）。
 *
 * 取代「每次自动保存弹一次 toast」的瞬时反馈：保存中/未保存/已保存/保存失败
 * 持续可见，失败提供就地重试，不被「关闭成功」视觉掩盖。聚合两个来源：
 * - 全局管线（use-aha-settings 的 loading/dirty/saveError）；
 * - 各页注册源（save-sources：SSH/MCP 自动保存、RAG 手动保存）——修复
 *   SSH 页保存失败时头部仍显示「已保存」的不一致。
 * 派生为单一语义状态（aggregateSaveStatuses 纯函数），零 store 契约改动。
 */
export function SaveStatusIndicator() {
  const store = useAhaSettings();
  const sources = useSyncExternalStore(subscribeSaveSources, getSaveSourcesSnapshot);
  const status = aggregateSaveStatuses(
    {
      loading: store.loading,
      dirty: store.dirty,
      hasError: store.saveError != null,
    },
    sources,
  );

  // 加载态由 panel-host 的「正在加载设置…」承担，头部不重复渲染。
  if (status === "loading") return null;

  if (status === "error") {
    return (
      <span className="ai-set-save-status is-error" role="status">
        <span className="ai-set-save-status-label">保存失败</span>
        <button
          type="button"
          className="ai-set-save-retry"
          onClick={() => {
            // 重试路由到两个来源：全局管线 flush + 全部注册源落盘
            // （错误源随 flush 重存，干净源幂等无多余写入面）。
            void store.flush();
            void flushAllSaveSources();
          }}
          title="重新保存"
        >
          <RotateCw size={12} strokeWidth={2} aria-hidden="true" />
          重试
        </button>
      </span>
    );
  }

  if (status === "saving") {
    return (
      <span className="ai-set-save-status is-saving" role="status">
        <Loader2 size={12} strokeWidth={2} className="ai-set-save-spin" aria-hidden="true" />
        保存中…
      </span>
    );
  }

  if (status === "unsaved") {
    return (
      <span className="ai-set-save-status is-unsaved" role="status">
        <PencilLine size={12} strokeWidth={2} aria-hidden="true" />
        未保存
      </span>
    );
  }

  return (
    <span className="ai-set-save-status is-saved" role="status">
      <Check size={12} strokeWidth={2} aria-hidden="true" />
      已保存
    </span>
  );
}
