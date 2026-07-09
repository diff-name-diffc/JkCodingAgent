import { Check, ChevronDown } from "lucide-react";
import type { DispatcherModelConfig } from "../../types";
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

/**
 * Compact model selector for the prompt footer.
 *
 * Lists chat model configs; the active one is marked. Selecting a model calls
 * `onSelect` with the model's *index* in the array — the backend
 * `aha_set_active_chat_model` command takes an index, not a name (see
 * src-tauri/src/agent/commands.rs).
 */
export interface ModelSelectorProps {
  models: DispatcherModelConfig[];
  onSelect: (modelIndex: number) => void;
  className?: string;
  disabled?: boolean;
}

export function ModelSelector({
  models,
  onSelect,
  className,
  disabled,
}: ModelSelectorProps) {
  const activeIndex = Math.max(
    0,
    models.findIndex((m) => m.active),
  );
  const active = models[activeIndex];

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled || models.length === 0}
          className={cn("gap-1.5 px-2 text-xs font-medium", className)}
          aria-label="选择模型"
        >
          <span className="max-w-[140px] truncate">
            {active?.model || "未配置模型"}
          </span>
          <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[220px]">
        <DropdownMenuLabel>聊天模型</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {models.length === 0 && (
          <div className="px-2 py-3 text-xs text-muted-foreground">
            暂无可用模型
          </div>
        )}
        {models.map((model, index) => (
          <DropdownMenuItem
            key={index}
            onClick={() => onSelect(index)}
            className="justify-between"
          >
            <span className="min-w-0 truncate">{model.model}</span>
            {model.active && <Check className="h-3.5 w-3.5 text-primary" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
