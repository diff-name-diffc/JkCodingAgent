import { Check, ChevronDown, Settings2 } from "lucide-react";
import type { ModelLibraryEntry } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { entryLabel } from "../settings/providers/model-library";

/**
 * Compact model selector for the prompt footer.
 *
 * 选项来自分类模型库（text 分类的启用条目）——与设置页「聊天主模型」下拉同一
 * 数据源。当前生效条目（聊天主模型用途绑定）打勾；选中后由 `onSelect` 回传条目
 * id，上层把它绑定为聊天主模型用途。
 */
export interface ModelSelectorProps {
  models: ModelLibraryEntry[];
  /** 当前生效的库条目 id（聊天主模型绑定匹配到的条目）。 */
  activeEntryId?: string;
  /** 触发按钮展示名；绑定指向库外旧配置时回退展示其模型名。 */
  activeLabel?: string;
  onSelect: (entryId: string) => void;
  /** 下拉菜单标题，默认「聊天模型」（架构助手等复用方传各自文案）。 */
  menuLabel?: string;
  className?: string;
  disabled?: boolean;
  /**
   * 无可用模型时的深链回调（UI-25）：直接打开设置「模型服务」页，取代旧的
   * 装饰性不可用按钮。必传——确保「未配置模型」始终是一个可操作的入口。
   */
  onConfigureModel: () => void;
}

export function ModelSelector({
  models,
  activeEntryId,
  activeLabel,
  onSelect,
  menuLabel = "聊天模型",
  className,
  disabled,
  onConfigureModel,
}: ModelSelectorProps) {
  // 无可用模型：不是禁用一个死按钮，而是给出「去配置」的深链入口（UI-25）。
  if (models.length === 0) {
    return (
      <Button
        variant="ghost"
        size="sm"
        onClick={onConfigureModel}
        className={cn(
          "ai-model-selector ai-model-selector--empty gap-2 px-2.5 text-xs font-medium",
          className,
        )}
        aria-label="尚未配置可用模型，前往设置"
        title="尚未配置可用模型，点击前往设置"
      >
        <Settings2 className="ai-model-selector-icon h-3.5 w-3.5" />
        <span className="max-w-[140px] truncate">配置模型</span>
      </Button>
    );
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          className={cn("ai-model-selector gap-2 px-2.5 text-xs font-medium", className)}
          aria-label="选择模型"
        >
          <Settings2 className="ai-model-selector-icon h-3.5 w-3.5" />
          <span className="max-w-[140px] truncate">{activeLabel || "未配置模型"}</span>
          <ChevronDown className="ai-model-selector-chevron h-3.5 w-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[220px]">
        <DropdownMenuLabel>{menuLabel}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {models.map((entry) => (
          <DropdownMenuItem
            key={entry.id}
            onClick={() => onSelect(entry.id)}
            className="justify-between"
          >
            <span className="min-w-0 truncate">{entryLabel(entry)}</span>
            {entry.id === activeEntryId && <Check className="h-3.5 w-3.5 text-primary" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
