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
  className?: string;
  disabled?: boolean;
}

export function ModelSelector({
  models,
  activeEntryId,
  activeLabel,
  onSelect,
  className,
  disabled,
}: ModelSelectorProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled || models.length === 0}
          className={cn("ai-model-selector gap-2 px-2.5 text-xs font-medium", className)}
          aria-label="选择模型"
        >
          <Settings2 className="ai-model-selector-icon h-3.5 w-3.5" />
          <span className="max-w-[140px] truncate">{activeLabel || "未配置模型"}</span>
          <ChevronDown className="ai-model-selector-chevron h-3.5 w-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[220px]">
        <DropdownMenuLabel>聊天模型</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {models.length === 0 && (
          <div className="px-2 py-3 text-xs text-muted-foreground">暂无可用模型</div>
        )}
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
