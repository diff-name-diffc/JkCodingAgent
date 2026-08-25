import { memo, useCallback, useMemo, useState } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { KeyRound, Lock } from "lucide-react";
import type { SshServerConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { Button } from "../../ui/button";

/** 导入条目行：memo 隔离，勾选某行时其余行跳过重渲染。 */
const ImportEntryRow = memo(function ImportEntryRow({
  index,
  entry,
  checked,
  duplicated,
  onToggle,
}: {
  index: number;
  entry: SshServerConfig;
  checked: boolean;
  duplicated: boolean;
  onToggle: (index: number) => void;
}) {
  const authLabel = entry.authMethod === "key" ? "私钥" : "密码";
  return (
    <label
      className={cn(
        "flex items-center gap-2 rounded-md px-2 py-1.5",
        duplicated ? "opacity-50" : "cursor-pointer hover:bg-[var(--bg-hover)]",
      )}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={duplicated}
        onChange={() => onToggle(index)}
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex items-center gap-1.5">
          <span className="truncate text-[13px] font-medium text-[var(--text-primary)]">
            {entry.name || entry.id}
          </span>
          {entry.authMethod === "key" ? (
            <KeyRound size={12} strokeWidth={1.5} className="flex-shrink-0 text-[var(--text-muted)]" />
          ) : (
            <Lock size={12} strokeWidth={1.5} className="flex-shrink-0 text-[var(--text-muted)]" />
          )}
          {duplicated && (
            <span className="flex-shrink-0 text-[11px] text-[var(--text-muted)]">
              已存在
            </span>
          )}
        </span>
        <span className="truncate text-[11.5px] text-[var(--text-secondary)]">
          {entry.username ? `${entry.username}@` : ""}
          {entry.host}
          {entry.port !== 22 ? `:${entry.port}` : ""}
          {` · ${authLabel}认证`}
        </span>
      </span>
    </label>
  );
});

/**
 * 「导入本机 SSH 配置」的选择对话框：列出从 ~/.ssh/config 解析出的主机草稿，
 * 用户勾选后由父组件合并进服务器列表并触发自动保存。
 * 与现有服务器重复（同 id 或同 user@host:port）的条目默认不勾选且不可选。
 */
export function SshImportDialog({
  entries,
  existing,
  onConfirm,
  onCancel,
}: {
  entries: SshServerConfig[];
  /** 现有服务器列表，用于判定重复条目。 */
  existing: SshServerConfig[];
  onConfirm: (selected: SshServerConfig[]) => void;
  onCancel: () => void;
}) {
  const duplicated = useMemo(() => {
    const ids = new Set(existing.map((server) => server.id));
    const endpoints = new Set(existing.map(endpointKey));
    return entries.map(
      (entry) => ids.has(entry.id) || endpoints.has(endpointKey(entry)),
    );
  }, [entries, existing]);

  const [selected, setSelected] = useState<Set<number>>(
    () => new Set(entries.map((_, index) => index).filter((index) => !duplicated[index])),
  );

  const handleToggle = useCallback((index: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }, []);

  const selectedCount = selected.size;

  return (
    <DialogPrimitive.Root
      open
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-set-confirm-overlay" />
        <DialogPrimitive.Content
          className="ai-set-confirm is-wide"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogPrimitive.Title className="ai-set-confirm-title">
            导入本机 SSH 配置
          </DialogPrimitive.Title>
          <DialogPrimitive.Description className="ai-set-confirm-description">
            从 ~/.ssh/config 解析出 {entries.length} 台主机，勾选要导入的条目；密码等凭据不会被导入，导入后可在列表中补充。
          </DialogPrimitive.Description>

          <div className="flex max-h-72 flex-col gap-1 overflow-y-auto py-1">
            {entries.map((entry, index) => (
              <ImportEntryRow
                key={`${entry.id}-${index}`}
                index={index}
                entry={entry}
                checked={selected.has(index)}
                duplicated={duplicated[index]}
                onToggle={handleToggle}
              />
            ))}
          </div>

          <div className="ai-set-confirm-actions">
            <Button variant="outline" size="sm" onClick={onCancel}>
              取消
            </Button>
            <Button
              size="sm"
              disabled={selectedCount === 0}
              onClick={() =>
                onConfirm(entries.filter((_, index) => selected.has(index)))
              }
            >
              导入 {selectedCount > 0 ? `（${selectedCount}）` : ""}
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

function endpointKey(server: SshServerConfig): string {
  return `${server.username.trim()}@${server.host.trim()}:${server.port}`;
}
