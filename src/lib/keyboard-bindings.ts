/**
 * keyboard-bindings.ts — 全局快捷键纯判定层（UI-23a）。
 *
 * 从 use-chat-shortcuts 抽出并扩展，node 环境可测：
 *   - isMacPlatform：Mod 键的平台映射（macOS=Cmd，其余=Ctrl）；
 *   - isImeKeyEvent：输入法组合期判定（isComposing / keyCode 229 / Process），
 *     组合期一律不触发全局快捷键，避免中文 IME 误触；
 *   - matchesBinding：事件与绑定（key/mod/shift/alt）的匹配；
 *   - shouldSkipBinding：统一裁决是否让路——
 *       ① IME 组合期全跳过；
 *       ② 非 Mac 且焦点在终端（.xterm）内时 Mod 键全放行给 shell
 *          （Mod=Ctrl，Ctrl+K/L/N/J 等是终端控制码，不能被 UI 抢走）；
 *       ③ 焦点在代码编辑器（.monaco-editor）内时，按绑定的 allowInEditor
 *          裁决（默认 true：应用级快捷键在编辑器内仍可触发）；
 *       ④ 输入目标（input/textarea/contentEditable）内不劫持非 Mod 键。
 *
 * DOM 只经 `target.closest(selector)` 结构化访问，测试用字面量桩即可。
 */

/** 与原生 KeyboardEvent 结构兼容的最小事件形状（target 放宽为 unknown 便于打桩）。 */
export interface KeyEventLike {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  isComposing?: boolean;
  keyCode?: number;
  which?: number;
  target?: unknown;
}

/** 事件目标的最小结构形状（Element 子集）。 */
export interface EventTargetLike {
  tagName?: string;
  isContentEditable?: boolean;
  closest?(selector: string): unknown;
}

/** 绑定的匹配字段（纯函数只依赖这部分，不含 handler，便于测试）。 */
export interface ShortcutModifiers {
  /** 单字符键统一小写比较；命名键（Escape 等）原样比较。 */
  key: string;
  /** Mod 表示 macOS 的 Cmd / 其他平台的 Ctrl。 */
  mod?: boolean;
  shift?: boolean;
  alt?: boolean;
  /**
   * 焦点位于代码编辑器（Monaco）内时是否仍触发。默认 true——应用级快捷键
   * （命令面板/切区等）优先于编辑器内部和弦；未来若注册编辑器敏感绑定，
   * 显式置 false 让路给 Monaco。
   */
  allowInEditor?: boolean;
  /** 命中后是否 preventDefault。默认 true；显式设 false 交由 handler 决定。 */
  preventDefault?: boolean;
}

export interface ShortcutBinding extends ShortcutModifiers {
  handler: (event: KeyboardEvent) => void;
}

/** 终端容器选择器（xterm.js 根节点）。 */
export const TERMINAL_CONTAINER_SELECTOR = ".xterm";
/** 代码编辑器容器选择器（Monaco 根节点）。 */
export const CODE_EDITOR_CONTAINER_SELECTOR = ".monaco-editor";

/** Mod 键平台映射：macOS（含 iOS）用 Cmd，其余用 Ctrl。navigator 缺失时按非 Mac。 */
export function isMacPlatform(
  nav: { platform?: string; userAgent?: string } | undefined = typeof navigator ===
  "undefined"
    ? undefined
    : { platform: navigator.platform, userAgent: navigator.userAgent },
): boolean {
  if (!nav) return false;
  // platform 与 userAgent 分开判定并加词边界：裸 /Mac/ 会被 Node.js 运行时的
  // userAgent（"Node.js/22.x"）等串误命中。
  if (/^(Mac|iPhone|iPad)/.test(nav.platform || "")) return true;
  return /\b(Mac OS X|Macintosh|iPhone|iPad)\b/.test(nav.userAgent || "");
}

/** 输入法组合期：此时任何全局快捷键都不应触发（含 Mod 组合）。 */
export function isImeKeyEvent(event: KeyEventLike): boolean {
  return Boolean(
    event.isComposing ||
      event.keyCode === 229 ||
      event.which === 229 ||
      event.key === "Process",
  );
}

/** 事件是否命中绑定的 key/mod/shift/alt（Mod 按平台映射到 metaKey/ctrlKey）。 */
export function matchesBinding(
  event: KeyEventLike,
  binding: ShortcutModifiers,
  mac: boolean,
): boolean {
  if (binding.mod !== undefined) {
    const hasMod = mac ? Boolean(event.metaKey) : Boolean(event.ctrlKey);
    if (binding.mod !== hasMod) return false;
  }
  if (binding.shift !== undefined && binding.shift !== Boolean(event.shiftKey)) return false;
  if (binding.alt !== undefined && binding.alt !== Boolean(event.altKey)) return false;
  // 单字符键归一小写；命名键（Escape/ArrowLeft…）原样比较。
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
  return key === binding.key;
}

/** 焦点是否在可输入目标内（input/textarea/contentEditable）。 */
export function isTypingTarget(target: EventTargetLike | null | undefined): boolean {
  if (!target) return false;
  return Boolean(
    target.tagName === "INPUT" ||
      target.tagName === "TEXTAREA" ||
      target.isContentEditable,
  );
}

/** 焦点目标是否位于指定选择器的祖先容器内。 */
export function isInsideContainer(
  target: EventTargetLike | null | undefined,
  selector: string,
): boolean {
  if (!target || typeof target.closest !== "function") return false;
  return target.closest(selector) != null;
}

/**
 * 统一让路裁决：返回 true 表示该绑定应跳过（把按键还给当前焦点上下文）。
 * 顺序即优先级：IME > 终端 > 编辑器 > 输入目标。
 */
export function shouldSkipBinding(
  event: KeyEventLike,
  binding: ShortcutModifiers,
  mac: boolean,
): boolean {
  if (isImeKeyEvent(event)) return true;
  const target = event.target as EventTargetLike | null | undefined;
  // 终端内（非 Mac）：Mod=Ctrl，Ctrl+K/L/N/J 等是 shell 控制码，全部放行；
  // Mac 的 Cmd 组合不会进入终端，应用级快捷键照常触发。
  if (binding.mod && !mac && isInsideContainer(target, TERMINAL_CONTAINER_SELECTOR)) {
    return true;
  }
  // 代码编辑器内：allowInEditor 显式关闭的绑定让路给 Monaco。
  if (
    binding.allowInEditor === false &&
    isInsideContainer(target, CODE_EDITOR_CONTAINER_SELECTOR)
  ) {
    return true;
  }
  // 输入目标内不劫持裸键（保留既有豁免语义）。
  if (!binding.mod && isTypingTarget(target)) return true;
  return false;
}
