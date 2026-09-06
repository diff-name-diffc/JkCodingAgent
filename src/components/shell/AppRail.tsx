import { Layers, MessageCircle, Workflow } from "lucide-react";
import { cn } from "../../lib/cn";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { SidebarFooterActions } from "../SidebarFooterActions";
import appLogo from "../../assets/app-logo.png";

export type HomeSpace = "chat" | "projects" | "architecture";
/** project = 项目工作台（已打开项目内），rail 高亮「项目」入口。 */
export type AppRailSpace = HomeSpace | "project";

interface AppRailProps {
  space: AppRailSpace;
  /** 欢迎页内部切视图；项目工作台传 onNavigateHome 代替。 */
  onSpaceChange?: (space: HomeSpace) => void;
  onNavigateHome?: (space: HomeSpace) => void;
  projectId?: string;
  projectPath?: string;
}

const ENTRIES: { key: HomeSpace; label: string; icon: typeof MessageCircle }[] = [
  { key: "chat", label: "聊天", icon: MessageCircle },
  { key: "projects", label: "项目", icon: Layers },
  { key: "architecture", label: "架构设计", icon: Workflow },
];

/**
 * 全局应用 rail（UI-07）：52px，三个工作空间入口 + 设置置底。
 * 欢迎页与项目工作台共用同一组件，保证互转入口位置一致；
 * 文本提示用 Tooltip 补足图标可发现性（替代旧 60px 图标栏的悬停标签）。
 */
export function AppRail({
  space,
  onSpaceChange,
  onNavigateHome,
  projectId,
  projectPath,
}: AppRailProps) {
  const activeKey: HomeSpace = space === "project" ? "projects" : space;
  const handle = (key: HomeSpace) => {
    if (space === "project") onNavigateHome?.(key);
    else onSpaceChange?.(key);
  };

  return (
    <div className="ai-app-rail">
      <div className="ai-app-rail-brand" aria-label="JKCodingAgent">
        <img src={appLogo} alt="JKCodingAgent" />
      </div>

      <nav className="ai-app-rail-list" aria-label="主导航">
        {ENTRIES.map(({ key, label, icon: Icon }) => (
          <Tooltip key={key}>
            <TooltipTrigger asChild>
              <button
                type="button"
                className={cn("ai-app-rail-item", activeKey === key && "is-active")}
                onClick={() => handle(key)}
                aria-label={label}
                aria-current={activeKey === key ? "page" : undefined}
              >
                <Icon size={18} strokeWidth={1.8} />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{label}</TooltipContent>
          </Tooltip>
        ))}
      </nav>

      <div className="ai-app-rail-footer">
        <SidebarFooterActions projectId={projectId} projectPath={projectPath} />
      </div>
    </div>
  );
}
