import type { ReactNode } from "react";
import { cn } from "../lib/cn";
import { Button } from "./ui/button";

/**
 * 旧图标按钮入口：UI-06 起包装 ui/Button（ghost + icon 尺寸），
 * 保留 .ai-icon-button 类供既有状态样式（is-active）与尺寸 prop 覆盖。
 */
export function IconButton({
  icon,
  title,
  active = false,
  disabled = false,
  onClick,
  size = 32,
}: {
  icon: ReactNode;
  title?: string;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  size?: number;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cn("ai-icon-button", active && "is-active")}
      style={{ width: size, height: size }}
    >
      {icon}
    </Button>
  );
}
