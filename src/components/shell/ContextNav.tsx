import { useCallback, useRef, useState } from "react";
import { ChevronsDown, PanelLeftClose, Plus, X } from "lucide-react";
import type { Project } from "../../types";
import { cn } from "../../lib/cn";
import { useSplitterKeyboard } from "../../hooks/use-splitter-keyboard";
import { isRovingKey, nextRovingIndex } from "../../lib/roving-index";
import { ProjectAvatar } from "../ProjectAvatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";

export type ContextNavTab = "sessions" | "files" | "changes" | "history";

const NAV_MIN = 216;
const NAV_MAX = 320;
const NAV_DEFAULT = 248;

interface ContextNavProps {
  project: Project;
  openProjects: Project[];
  allProjects: Project[];
  activeTab: ContextNavTab;
  onTabChange: (tab: ContextNavTab) => void;
  width: number;
  /** 拖拽结束才写回（持久化归属见 UI-08）。 */
  onWidthCommit: (width: number) => void;
  onSwitchProject: (project: Project) => void;
  onCloseProject: (project: Project) => void;
  onOpenProject: () => void;
  onCollapse?: () => void;
  sessionContent: React.ReactNode;
  filesContent: React.ReactNode;
  changesContent: React.ReactNode;
  /** Git 历史为变更域次级页签（02-design §2），保留既有提交/文件 diff 入口。 */
  historyContent: React.ReactNode;
}

const TABS: { key: ContextNavTab; label: string }[] = [
  { key: "sessions", label: "会话" },
  { key: "files", label: "文件" },
  { key: "changes", label: "变更" },
  { key: "history", label: "历史" },
];

/**
 * 上下文导航（UI-07）：项目切换器 + 会话/文件/变更三互斥页签 + 对象列表。
 * 默认 248px、可拖 216–320；替代旧 288px 会话栏与右栏的文件/Git 入口。
 */
export function ContextNav({
  project,
  openProjects,
  allProjects,
  activeTab,
  onTabChange,
  width,
  onWidthCommit,
  onSwitchProject,
  onCloseProject,
  onOpenProject,
  onCollapse,
  sessionContent,
  filesContent,
  changesContent,
  historyContent,
}: ContextNavProps) {
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const latest = useRef(width);

  const startResize = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      event.preventDefault();
      const startX = event.clientX;
      const startWidth = dragWidth ?? width;
      latest.current = startWidth;
      const prevCursor = document.body.style.cursor;
      const prevUserSelect = document.body.style.userSelect;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      const handleMove = (e: PointerEvent) => {
        const next = Math.min(NAV_MAX, Math.max(NAV_MIN, startWidth + e.clientX - startX));
        latest.current = next;
        setDragWidth(next);
      };
      const handleUp = () => {
        window.removeEventListener("pointermove", handleMove);
        window.removeEventListener("pointerup", handleUp);
        document.body.style.cursor = prevCursor;
        document.body.style.userSelect = prevUserSelect;
        onWidthCommit(latest.current);
        setDragWidth(null);
      };
      window.addEventListener("pointermove", handleMove);
      window.addEventListener("pointerup", handleUp);
    },
    [dragWidth, width, onWidthCommit],
  );

  const renderedWidth = dragWidth ?? width;
  // tablist 方向键（UI-23d）：roving tabindex + automatic activation——方向键
  // 移动焦点即切换页签（与点击语义一致，四页签内容均为已渲染列表，无昂贵加载）。
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const activeTabIndex = Math.max(
    0,
    TABS.findIndex((tab) => tab.key === activeTab),
  );
  const handleTabListKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      if (!isRovingKey(event.key, "horizontal")) return;
      event.preventDefault();
      const next = nextRovingIndex({
        count: TABS.length,
        current: activeTabIndex,
        key: event.key,
        wrap: true,
        orientation: "horizontal",
      });
      if (next === activeTabIndex) return;
      onTabChange(TABS[next].key);
      tabRefs.current[next]?.focus();
    },
    [activeTabIndex, onTabChange],
  );
  // 导航宽度把手键盘化（UI-23c）：ArrowLeft/Right 步进、Shift 大步、双击复位。
  const splitterKeyboard = useSplitterKeyboard({
    orientation: "vertical",
    mode: "px",
    ariaLabel: "方向键调整导航宽度，双击恢复默认",
    getValue: () => renderedWidth,
    getBounds: () => ({ min: NAV_MIN, max: NAV_MAX }),
    getDefaultValue: () => NAV_DEFAULT,
    onCommit: onWidthCommit,
  });
  const closedProjects = allProjects.filter((p) => !openProjects.some((o) => o.id === p.id));

  return (
    <div className="ai-context-nav" style={{ width: renderedWidth }}>
      <div className="ai-context-nav-header ai-context-nav-header-row">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className="ai-context-nav-switcher" title="切换项目">
              <ProjectAvatar name={project.name} size={20} />
              <span className="ai-context-nav-switcher-name">{project.name}</span>
              <ChevronsDown size={14} strokeWidth={2} />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="ai-context-nav-menu">
            {openProjects.map((p) => (
              <DropdownMenuItem key={p.id} onSelect={() => onSwitchProject(p)}>
                <span className="flex min-w-0 flex-1 items-center gap-2">
                  <ProjectAvatar name={p.name} size={16} />
                  <span className="truncate">{p.name}</span>
                </span>
                {p.id === project.id ? null : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span
                        role="button"
                        tabIndex={0}
                        aria-label={`关闭工作区 ${p.name}`}
                        className="ai-context-nav-close"
                        onClick={(e) => {
                          e.stopPropagation();
                          onCloseProject(p);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.stopPropagation();
                            onCloseProject(p);
                          }
                        }}
                      >
                        <X size={12} strokeWidth={2} />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent>关闭工作区（不删除项目）</TooltipContent>
                  </Tooltip>
                )}
              </DropdownMenuItem>
            ))}
            {closedProjects.length > 0 && <DropdownMenuSeparator />}
            {closedProjects.map((p) => (
              <DropdownMenuItem key={p.id} onSelect={() => onSwitchProject(p)}>
                <span className="truncate text-muted-foreground">{p.name}</span>
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onOpenProject}>
              <Plus size={14} strokeWidth={2} />
              打开项目…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {onCollapse && (
          <button
            type="button"
            className="ai-context-nav-collapse"
            aria-label="收起导航"
            title="收起导航"
            onClick={onCollapse}
          >
            <PanelLeftClose size={14} strokeWidth={2} />
          </button>
        )}
      </div>

      <div
        className="ai-context-nav-tabs"
        role="tablist"
        aria-label="导航内容"
        onKeyDown={handleTabListKeyDown}
      >
        {TABS.map(({ key, label }, index) => (
          <button
            key={key}
            ref={(element) => {
              tabRefs.current[index] = element;
            }}
            type="button"
            role="tab"
            id={`ctxnav-tab-${key}`}
            aria-controls="ctxnav-panel"
            aria-selected={activeTab === key}
            tabIndex={activeTab === key ? 0 : -1}
            className={cn("ai-context-nav-tab", activeTab === key && "is-active")}
            onClick={() => onTabChange(key)}
          >
            {label}
          </button>
        ))}
      </div>

      <div
        className="ai-context-nav-content"
        role="tabpanel"
        id="ctxnav-panel"
        aria-labelledby={`ctxnav-tab-${activeTab}`}
      >
        {activeTab === "sessions" && sessionContent}
        {activeTab === "files" && filesContent}
        {activeTab === "changes" && changesContent}
        {activeTab === "history" && historyContent}
      </div>

      <div
        {...splitterKeyboard}
        data-dragging={dragWidth != null}
        onPointerDown={startResize}
        className="ai-sidebar-resize-handle"
      />
    </div>
  );
}
