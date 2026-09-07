import * as React from "react";

/**
 * row-ui-state.ts — 消息行级瞬时 UI 状态存储（UI-24b-1）。
 *
 * 背景：>300 条消息窗口化（react-virtual 动态测量）后，滚出窗口的行会真实
 * 卸载；行内组件自持的 useState（工具卡组展开/单卡展开/输出「展开全部」）
 * 随卸载丢失，滚回来时卡片被重置为默认折叠——用户视角是「展开态被吃掉」。
 *
 * 方案：MessageList 实例持有一个 ref-backed 的不透明 key-value store
 * （不触发任何重渲染），经 Context 下发；行内组件用 usePersistedToggle 把
 * 布尔态镜像进去——挂载时从 store 恢复初值，切换时写回。store 生命周期与
 * MessageList 实例一致（会话视图存活期间），行卸载不影响。
 *
 * 边界：
 * - 仅承载「重挂载后应恢复」的瞬时 UI 布尔态，禁止放业务数据；
 * - key 由调用方保证稳定且实例内唯一（如 `tools:${turnId}`、`card:${toolCallId}`）；
 * - 流式气泡（StreamingMessage）不传 key，保持组件内临时态语义不变。
 */

export interface RowUiStateStore {
  get(key: string): boolean | undefined;
  set(key: string, value: boolean): void;
}

export function createRowUiStateStore(): RowUiStateStore {
  const map = new Map<string, boolean>();
  return {
    get: (key) => map.get(key),
    set: (key, value) => {
      map.set(key, value);
    },
  };
}

const RowUiStateContext = React.createContext<RowUiStateStore | null>(null);

export const RowUiStateProvider = RowUiStateContext.Provider;

/** MessageList 实例级 store：ref 持有，identity 恒定，不引发重渲染。 */
export function useRowUiStateStore(): RowUiStateStore {
  const ref = React.useRef<RowUiStateStore | null>(null);
  if (ref.current === null) ref.current = createRowUiStateStore();
  return ref.current;
}

/**
 * 带持久化镜像的布尔态：storeKey 存在时初值从 store 恢复、切换写回；
 * 缺省（或无 Provider，如独立使用 MessageItem 的测试场景）时退化为
 * 普通 useState，语义与迁移前完全一致。
 */
export function usePersistedToggle(
  storeKey: string | undefined,
  defaultValue: boolean,
): [boolean, (next: boolean | ((prev: boolean) => boolean)) => void] {
  const store = React.useContext(RowUiStateContext);
  const [value, setValue] = React.useState<boolean>(() => {
    if (storeKey === undefined || store === null) return defaultValue;
    return store.get(storeKey) ?? defaultValue;
  });
  const keyRef = React.useRef(storeKey);
  keyRef.current = storeKey;
  const set = React.useCallback(
    (next: boolean | ((prev: boolean) => boolean)) => {
      setValue((prev) => {
        const resolved = typeof next === "function" ? next(prev) : next;
        if (keyRef.current !== undefined && store !== null) {
          store.set(keyRef.current, resolved);
        }
        return resolved;
      });
    },
    [store],
  );
  return [value, set];
}
