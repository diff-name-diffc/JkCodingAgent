import * as React from "react";
import {
  MessageSquarePlus,
  PanelLeft,
  Search,
  Settings,
  TerminalSquare,
} from "lucide-react";
import type { ChatSession } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Input } from "../ui/input";

export interface CommandPaletteProps {
  open: boolean;
  sessions: ChatSession[];
  onOpenChange: (open: boolean) => void;
  onNewConversation: () => void;
  onSelectSession: (id: string) => void;
  onFocusPrompt: () => void;
  onToggleSidebar: () => void;
  onOpenSettings: () => void;
}

export function CommandPalette({
  open,
  sessions,
  onOpenChange,
  onNewConversation,
  onSelectSession,
  onFocusPrompt,
  onToggleSidebar,
  onOpenSettings,
}: CommandPaletteProps) {
  const [query, setQuery] = React.useState("");
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (!open) return;
    setQuery("");
    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [open]);

  React.useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onOpenChange(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onOpenChange, open]);

  if (!open) return null;

  const normalizedQuery = query.trim().toLowerCase();
  const filteredSessions = normalizedQuery
    ? sessions.filter((session) => session.title.toLowerCase().includes(normalizedQuery))
    : sessions.slice(0, 8);

  const run = (action: () => void) => {
    action();
    onOpenChange(false);
  };

  return (
    <div className="fixed inset-0 z-50 bg-background/50 backdrop-blur-sm" role="presentation">
      <button
        type="button"
        aria-label="关闭命令面板"
        className="absolute inset-0 cursor-default"
        onClick={() => onOpenChange(false)}
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
        className="absolute left-1/2 top-[12vh] flex w-[min(620px,calc(100vw-32px))] -translate-x-1/2 flex-col overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground shadow-strong"
      >
        <div className="flex items-center gap-2 border-b border-border px-3 py-2">
          <Search className="h-4 w-4 text-muted-foreground" />
          <Input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索会话或输入动作"
            className="h-9 border-0 bg-transparent px-0 focus-visible:ring-0"
          />
          <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground">
            Esc
          </kbd>
        </div>

        <div className="max-h-[60vh] overflow-y-auto p-2">
          <CommandSection title="动作">
            <CommandButton
              icon={<MessageSquarePlus />}
              label="新建对话"
              shortcut="⌘N"
              onClick={() => run(onNewConversation)}
            />
            <CommandButton
              icon={<TerminalSquare />}
              label="聚焦输入框"
              shortcut="⌘L"
              onClick={() => run(onFocusPrompt)}
            />
            <CommandButton
              icon={<PanelLeft />}
              label="切换侧边栏"
              shortcut="⌘B"
              onClick={() => run(onToggleSidebar)}
            />
            <CommandButton
              icon={<Settings />}
              label="打开设置"
              onClick={() => run(onOpenSettings)}
            />
          </CommandSection>

          <CommandSection title="会话">
            {filteredSessions.length === 0 ? (
              <div className="px-2 py-6 text-center text-xs text-muted-foreground">
                没有匹配的会话
              </div>
            ) : (
              filteredSessions.map((session) => (
                <CommandButton
                  key={session.id}
                  label={session.title.trim() || "新对话"}
                  meta={new Date(session.updatedAt).toLocaleString()}
                  onClick={() => run(() => onSelectSession(session.id))}
                />
              ))
            )}
          </CommandSection>
        </div>
      </div>
    </div>
  );
}

function CommandSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="mb-2">
      <div className="px-2 py-1.5 text-[11px] font-medium uppercase text-muted-foreground">
        {title}
      </div>
      <div className="space-y-1">{children}</div>
    </section>
  );
}

function CommandButton({
  icon,
  label,
  meta,
  shortcut,
  onClick,
}: {
  icon?: React.ReactNode;
  label: string;
  meta?: string;
  shortcut?: string;
  onClick: () => void;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      className="h-auto w-full justify-start px-2 py-2 text-left"
      onClick={onClick}
    >
      {icon && (
        <span className="flex h-6 w-6 shrink-0 items-center justify-center text-muted-foreground [&_svg]:h-4 [&_svg]:w-4">
          {icon}
        </span>
      )}
      <span className={cn("min-w-0 flex-1", !icon && "pl-1")}>
        <span className="block truncate text-sm text-foreground">{label}</span>
        {meta && <span className="block truncate text-xs text-muted-foreground">{meta}</span>}
      </span>
      {shortcut && (
        <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground">
          {shortcut}
        </kbd>
      )}
    </Button>
  );
}
