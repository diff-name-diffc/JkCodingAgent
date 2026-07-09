import { Moon, Sun, Monitor } from "lucide-react";
import type { ThemeMode } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

/**
 * Theme toggle for the refactored Chat surface.
 *
 * Mirrors the existing App.tsx theme logic: the actual `html.dark` toggle and
 * native window setTheme stay in App.tsx (so the Radix Themes Theme wrapper
 * stays in sync). This component only calls `onChange` with the user's choice;
 * the parent is responsible for applying it.
 *
 * Renders a single icon button; a small dropdown picks between light / dark /
 * system.
 */
export interface ThemeToggleProps {
  theme: ThemeMode;
  isDark: boolean;
  onChange: (mode: ThemeMode) => void;
  className?: string;
}

export function ThemeToggle({ theme, isDark, onChange, className }: ThemeToggleProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="切换主题"
          className={cn(className)}
        >
          {theme === "system" ? (
            <Monitor className="h-4 w-4" />
          ) : isDark ? (
            <Moon className="h-4 w-4" />
          ) : (
            <Sun className="h-4 w-4" />
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => onChange("light")}>
          <Sun className="h-4 w-4" /> 浅色
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onChange("dark")}>
          <Moon className="h-4 w-4" /> 深色
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onChange("system")}>
          <Monitor className="h-4 w-4" /> 跟随系统
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
