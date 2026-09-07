/**
 * Git diff 视图模式偏好（UI-17 遗留：split 模式）。
 *
 * unified / split 的选择是全局 UI 偏好（无对应存储字段、不随项目/会话变化），
 * 故只存 localStorage，换设备不同步——模式参照 settings/providers/provider-prefs。
 * 校验逻辑抽为纯函数 `sanitizeDiffViewMode`，storage 以最小接口注入，便于在
 * node 测试环境（vitest environment=node，无 window.localStorage）验证回退。
 */

export type DiffViewMode = "unified" | "split";

const DIFF_VIEW_MODE_KEY = "jkcodingagent.git.diffViewMode.v1";

/** localStorage 形态的最小子集；可注入 mock 供测试。 */
export interface DiffViewStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** 纯校验：非法/未知值回退 "unified"（默认，设计 §5.4「窄区域允许 unified」）。 */
export function sanitizeDiffViewMode(value: unknown): DiffViewMode {
  return value === "split" ? "split" : "unified";
}

function defaultStorage(): DiffViewStorage | null {
  try {
    return typeof window !== "undefined" && window.localStorage
      ? (window.localStorage as DiffViewStorage)
      : null;
  } catch {
    return null;
  }
}

export function loadDiffViewMode(
  storage: DiffViewStorage | null = defaultStorage(),
): DiffViewMode {
  if (!storage) return "unified";
  try {
    return sanitizeDiffViewMode(storage.getItem(DIFF_VIEW_MODE_KEY));
  } catch {
    return "unified";
  }
}

export function saveDiffViewMode(
  mode: DiffViewMode,
  storage: DiffViewStorage | null = defaultStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(DIFF_VIEW_MODE_KEY, sanitizeDiffViewMode(mode));
  } catch {
    // localStorage 不可用（隐私模式等）时静默降级为会话内行为。
  }
}
