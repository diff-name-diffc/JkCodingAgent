import { Check, Loader2, RotateCw } from "lucide-react";
import { useAhaSettings } from "./use-aha-settings";
import { deriveSaveStatus } from "./save-status";

/**
 * 设置内容头部的持续保存状态指示器（UI-21）。
 *
 * 取代「每次自动保存弹一次 toast」的瞬时反馈：保存中/已保存/保存失败持续可见，
 * 失败提供就地重试（复用 store.flush 立即保存管线），不被「关闭成功」视觉掩盖。
 * 派生自现有 store 快照（loading/dirty/saveError），零 store 契约改动。
 */
export function SaveStatusIndicator() {
  const store = useAhaSettings();
  const status = deriveSaveStatus({
    loading: store.loading,
    dirty: store.dirty,
    hasError: store.saveError != null,
  });

  // 加载态由 panel-host 的「正在加载设置…」承担，头部不重复渲染。
  if (status === "loading") return null;

  if (status === "error") {
    return (
      <span className="ai-set-save-status is-error" role="status">
        <span className="ai-set-save-status-label">保存失败</span>
        <button
          type="button"
          className="ai-set-save-retry"
          onClick={() => void store.flush()}
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

  return (
    <span className="ai-set-save-status is-saved" role="status">
      <Check size={12} strokeWidth={2} aria-hidden="true" />
      已保存
    </span>
  );
}
