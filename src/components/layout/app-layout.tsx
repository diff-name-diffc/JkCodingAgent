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
 *   │ (260px / │  (flex-1, max-w prose      │  (optional,  │
 *   │  56px)   │   column centered)         │   resizable) │
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
  /** Custom titlebar drag region. Pass a thin bar so the window is draggable. */
  titlebar?: React.ReactNode;
  embedded?: boolean;
}

const SIDEBAR_WIDE = 264;
const SIDEBAR_NARROW = 56;

export function AppLayout({
  sidebar,
  chatHeader,
  children,
  chatFooter,
  artifactPanel,
  titlebar,
}: AppLayoutProps) {
  const collapsed = useUIStore((s) => s.sidebarCollapsed);
  const artifactOpen = useUIStore((s) => s.artifactPanelOpen);

  return (
    <div
      className={cn(
        "ai-chat-shell flex flex-col overflow-hidden bg-background text-foreground",
        "h-full w-full",
      )}
    >
      {titlebar}
      <div className="flex min-h-0 flex-1">
        {sidebar && (
          <motion.aside
            animate={{ width: collapsed ? SIDEBAR_NARROW : SIDEBAR_WIDE }}
            transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
            className={cn(
              "ai-chat-sidebar relative z-20 flex h-full shrink-0 flex-col border-r border-sidebar-border bg-sidebar",
            )}
          >
            {sidebar}
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
            <div className="ai-chat-footer sticky bottom-0 z-10 px-4 pb-3 pt-2">
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
