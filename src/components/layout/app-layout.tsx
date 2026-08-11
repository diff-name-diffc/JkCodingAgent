import * as React from "react";
import { AnimatePresence, motion } from "framer-motion";
import { useUIStore } from "../../stores/ui-store";
import { cn } from "../../lib/cn";

/**
 * Top-level desktop layout for the refactored Chat surface.
 *
 * Three regions:
 *   ┌──────────┬──────────────────────────┬──────────────┐
 *   │ Sidebar  │  Chat main area           │  Artifact    │
 *   │ (resizable│  (flex-1, max-w prose     │  (optional,  │
 *   │  / 56px) │   column centered)         │   resizable) │
 *   └──────────┴──────────────────────────┴──────────────┘
 *
 * The sidebar collapses to an icon rail; the artifact panel is opt-in and
 * overlay-style so it never squeezes the chat reading column.
 *
 * This component owns NO business logic — it only arranges children and reads
 * layout flags from the Zustand UI store. Data lives in <Sidebar /> and
 * <ChatShell />.
 */
export interface AppLayoutProps {
  sidebar?: React.ReactNode;
  /** Header rendered above the messages, sticky. */
  chatHeader?: React.ReactNode;
  children: React.ReactNode;
  /** Sticky footer (the prompt input). */
  chatFooter?: React.ReactNode;
  artifactPanel?: React.ReactNode;
}

const SIDEBAR_NARROW = 56;
const SIDEBAR_DEFAULT = 264;
const SIDEBAR_MIN = 200;
const SIDEBAR_MAX = 480;

export function AppLayout({
  sidebar,
  chatHeader,
  children,
  chatFooter,
  artifactPanel,
}: AppLayoutProps) {
  const collapsed = useUIStore((s) => s.sidebarCollapsed);
  const sidebarWidth = useUIStore((s) => s.sidebarWidth);
  const setSidebarWidth = useUIStore((s) => s.setSidebarWidth);
  const artifactOpen = useUIStore((s) => s.artifactPanelOpen);
  /** 拖拽中的实时宽度；为 null 表示未在拖拽。拖拽结束才写回 store，避免高频持久化。 */
  const [dragWidth, setDragWidth] = React.useState<number | null>(null);

  const startSidebarResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = dragWidth ?? sidebarWidth;
    const latest = { current: startWidth };
    const prevCursor = document.body.style.cursor;
    const prevUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const handleMove = (e: PointerEvent) => {
      const next = Math.min(
        SIDEBAR_MAX,
        Math.max(SIDEBAR_MIN, startWidth + e.clientX - startX),
      );
      latest.current = next;
      setDragWidth(next);
    };
    const handleUp = () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      document.body.style.cursor = prevCursor;
      document.body.style.userSelect = prevUserSelect;
      setSidebarWidth(latest.current);
      setDragWidth(null);
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
  };

  return (
    <div
      className={cn(
        "ai-chat-shell flex flex-col overflow-hidden bg-background text-foreground",
        "h-full w-full",
      )}
    >
      <div className="flex min-h-0 flex-1">
        {sidebar && (
          <motion.aside
            animate={{
              width: collapsed ? SIDEBAR_NARROW : (dragWidth ?? sidebarWidth),
            }}
            transition={
              dragWidth != null
                ? { duration: 0 }
                : { duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }
            }
            className={cn(
              "ai-chat-sidebar relative z-20 flex h-full shrink-0 flex-col border-r border-sidebar-border bg-sidebar",
            )}
          >
            {sidebar}
            {!collapsed && (
              <div
                role="separator"
                aria-orientation="vertical"
                aria-label="拖拽调整侧边栏宽度，双击恢复默认"
                data-dragging={dragWidth != null}
                onPointerDown={startSidebarResize}
                onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT)}
                className="ai-sidebar-resize-handle"
              />
            )}
          </motion.aside>
        )}

        <main className="ai-chat-main relative flex min-w-0 flex-1 flex-col">
          {/* Chat surface: a centered reading column within a full-height flex. */}
          {chatHeader && (
            <div className="ai-chat-header sticky top-0 z-10 border-b border-border/60 bg-background/80 backdrop-blur supports-[backdrop-filter]:bg-background/60">
              {chatHeader}
            </div>
          )}
          <div className="ai-chat-stage relative flex min-h-0 flex-1 justify-center">
            <div className="ai-chat-column flex min-h-0 flex-1 flex-col">
              {children}
            </div>
          </div>
          {chatFooter && (
            // 无 px-4、也不再包一层 .ai-chat-column：footer 内容（PromptInput）
            // 根节点自带 .ai-chat-column 的列宽档位与居中逻辑；外层再套一层会
            // 让 `min(…, 100% - 48px)` 基于外层列宽二次收缩，窄窗口下输入框
            // 反而比消息列窄 48px。外层只提供 sticky 与渐隐背景。
            <div className="ai-chat-footer sticky bottom-0 z-10 pb-3 pt-2">
              {chatFooter}
            </div>
          )}
        </main>

        <AnimatePresence initial={false}>
          {artifactOpen && artifactPanel && (
            <motion.section
              key="artifact"
              initial={{ width: 0, opacity: 0 }}
              animate={{ width: 420, opacity: 1 }}
              exit={{ width: 0, opacity: 0 }}
              transition={{ duration: 0.2, ease: [0.2, 0.8, 0.2, 1] }}
              className="ai-artifact-dock relative z-20 h-full shrink-0 overflow-hidden border-l border-border bg-card"
            >
              <div className="h-full w-[420px]">{artifactPanel}</div>
            </motion.section>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
