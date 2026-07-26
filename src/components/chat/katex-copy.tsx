import { useCallback, useEffect, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";

/**
 * KaTeX 渲染结果的「智能复制」交互：
 *   - 单击公式（未发生拖选时）→ 直接复制原始 LaTeX 源码（方案 A）
 *   - 右键公式 → 上下文菜单「复制公式源码」（方案 D，可发现性兜底）
 *   - 拖选公式保持普通文本选择行为不变，优先于点击复制
 *
 * 原始 LaTeX 取自 KaTeX 输出中内嵌的
 * `<annotation encoding="application/x-tex">`，无需额外状态。
 */

interface KatexTarget {
  tex: string;
  element: HTMLElement;
}

interface KatexMenuState extends KatexTarget {
  x: number;
  y: number;
}

/** 菜单大致尺寸，用于把弹出位置约束在视口内。 */
const MENU_WIDTH = 160;
const MENU_HEIGHT = 48;
const COPIED_FLASH_CLASS = "ai-katex-copied";
const COPIED_FLASH_MS = 700;

function findKatexTarget(target: EventTarget | null): KatexTarget | null {
  if (!(target instanceof HTMLElement)) return null;
  const katex = target.closest<HTMLElement>(".katex");
  if (!katex) return null;
  const tex = katex
    .querySelector('annotation[encoding="application/x-tex"]')
    ?.textContent?.trim();
  if (!tex) return null;
  return { tex, element: katex };
}

function flashCopied(element: HTMLElement) {
  element.classList.add(COPIED_FLASH_CLASS);
  window.setTimeout(() => element.classList.remove(COPIED_FLASH_CLASS), COPIED_FLASH_MS);
}

function copyKatexTex({ tex, element }: KatexTarget) {
  navigator.clipboard
    .writeText(tex)
    .then(() => flashCopied(element))
    .catch(() => {
      // 剪贴板权限被拒等场景静默失败，用户仍可拖选复制渲染文本。
    });
}

export interface KatexCopyContainerProps {
  onClick: (event: ReactMouseEvent<HTMLElement>) => void;
  onContextMenu: (event: ReactMouseEvent<HTMLElement>) => void;
  onMouseOver: (event: ReactMouseEvent<HTMLElement>) => void;
}

export function useKatexCopy(): {
  containerProps: KatexCopyContainerProps;
  menuElement: React.ReactNode;
} {
  const [menu, setMenu] = useState<KatexMenuState | null>(null);

  const onClick = useCallback((event: ReactMouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    // 拖选优先：选区非空时不打断正常的文本选择/自动复制。
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    const found = findKatexTarget(event.target);
    if (found) copyKatexTex(found);
  }, []);

  const onContextMenu = useCallback((event: ReactMouseEvent<HTMLElement>) => {
    const found = findKatexTarget(event.target);
    if (!found) return;
    event.preventDefault();
    setMenu({
      ...found,
      x: Math.min(event.clientX, window.innerWidth - MENU_WIDTH),
      y: Math.min(event.clientY, window.innerHeight - MENU_HEIGHT),
    });
  }, []);

  // 悬停时补一个原生 tooltip 作为可发现性提示（只设置一次）。
  const onMouseOver = useCallback((event: ReactMouseEvent<HTMLElement>) => {
    const katex =
      event.target instanceof HTMLElement
        ? event.target.closest<HTMLElement>(".katex")
        : null;
    if (katex && !katex.title) {
      katex.title = "点击复制公式源码";
    }
  }, []);

  // 菜单打开期间：点击外部 / Escape / 滚动 / 缩放窗口时关闭。
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    const onMouseDown = (event: globalThis.MouseEvent) => {
      if (event.target instanceof HTMLElement && event.target.closest(".ai-katex-menu")) {
        return;
      }
      close();
    };
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [menu]);

  const menuElement = menu
    ? createPortal(
        <div className="ai-katex-menu" role="menu" style={{ left: menu.x, top: menu.y }}>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              copyKatexTex(menu);
              setMenu(null);
            }}
          >
            复制公式源码
          </button>
        </div>,
        document.body,
      )
    : null;

  return { containerProps: { onClick, onContextMenu, onMouseOver }, menuElement };
}
