import { nextRovingIndex } from "./roving-index";

/**
 * list-focus.ts — 列表方向键焦点移动的 DOM 薄层（UI-23d）。
 *
 * 索引数学在 lib/roving-index（node 可测）；本模块只做「查项目 → 定位当前
 * 焦点 → 移动 focus + scrollIntoView」。用于导航列表（会话列表/侧栏/命令
 * 面板）——列表项保持天然 tabbable（li>button），不引入 roving tabindex；
 * tablist（ContextNav）用 roving tabindex 语义，不走本工具。
 */

export interface MoveListFocusOptions {
  /** 项目选择器（相对容器）。 */
  selector: string;
  /** 焦点不在列表内时的入列方向键行为：ArrowDown→首项 / ArrowUp→末项。 */
  wrap?: boolean;
}

/**
 * 在容器内沿 key（↑↓/Home/End）移动焦点。返回 true 表示已处理（调用方
 * preventDefault）；false 表示无项目或越界未动（wrap=false 的边界）。
 */
export function moveListFocus(
  container: HTMLElement | null,
  key: string,
  options: MoveListFocusOptions,
): boolean {
  if (!container) return false;
  const items = Array.from(
    container.querySelectorAll<HTMLElement>(options.selector),
  ).filter((element) => !element.hasAttribute("disabled"));
  if (items.length === 0) return false;

  const activeIndex = items.indexOf(document.activeElement as HTMLElement);
  let target: HTMLElement | undefined;
  if (activeIndex === -1) {
    // 焦点在列表外（如搜索框）：ArrowDown 入首项、ArrowUp 入末项。
    if (key === "ArrowDown" || key === "Home") target = items[0];
    else if (key === "ArrowUp" || key === "End") target = items[items.length - 1];
    else return false;
  } else {
    const next = nextRovingIndex({
      count: items.length,
      current: activeIndex,
      key,
      wrap: options.wrap ?? true,
      orientation: "vertical",
    });
    if (next === activeIndex) return false;
    target = items[next];
  }
  if (!target) return false;
  target.focus();
  target.scrollIntoView({ block: "nearest" });
  return true;
}
