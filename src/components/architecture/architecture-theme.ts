import type { Editor } from "tldraw";

/** 应用侧已把 system 解析为确定的亮/暗，画布只接收确定值。 */
export function resolveTldrawColorScheme(dark: boolean): "dark" | "light" {
  return dark ? "dark" : "light";
}

/**
 * 命令式同步画布主题（随应用亮/暗切换）。
 *
 * 不要用 `<Tldraw colorScheme={...}>` prop：v5 中 `colorScheme` 位于创建
 * Editor 的 effect 依赖数组，prop 变化会整体重建画布（丢失视口位置与撤销栈）。
 * `updateUserPreferences` 是响应式的，不重建。
 */
export function applyTldrawColorScheme(editor: Editor, dark: boolean): void {
  editor.user.updateUserPreferences({ colorScheme: resolveTldrawColorScheme(dark) });
}
