import { useCallback, useEffect, useState } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { invoke } from "@tauri-apps/api/core";
import type { SshMemoPayload } from "../../../types";
import { Button } from "../../ui/button";
import { Textarea } from "../../ui/textarea";

/** 与后端 `ssh_tool::memo::MEMO_MAX_CHARS` 一致；权威校验在后端，此处仅提示。 */
const MEMO_MAX_CHARS = 8000;

/**
 * 单台 SSH 服务器的运维备忘录编辑对话框。
 *
 * 备忘录本体是 `~/.jkcodingagent/ssh-memos/{server_id}.md`，由智能体在运维
 * 中通过 ssh_memo_* 工具读写；本对话框提供人工查看与纠正入口，保存走与
 * Agent 工具同一存储与上限约束（全文 8000 / 单段 4000 字符，超限拒绝）。
 */
export function SshMemoDialog({
  serverId,
  serverLabel,
  onClose,
}: {
  serverId: string;
  serverLabel: string;
  onClose: () => void;
}) {
  const [content, setContent] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>();

  useEffect(() => {
    let cancelled = false;
    invoke<SshMemoPayload>("ssh_tool_get_memo", { serverId })
      .then((payload) => {
        if (cancelled) return;
        setContent(payload.content);
        setLoading(false);
      })
      .catch((unknownError) => {
        if (cancelled) return;
        setError(String(unknownError));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [serverId]);

  const save = useCallback(async () => {
    setSaving(true);
    setError(undefined);
    try {
      // 后端会规范化内容（头部注释、段落格式）并做上限校验，以返回值为准。
      const payload = await invoke<SshMemoPayload>("ssh_tool_save_memo", {
        serverId,
        content,
      });
      setContent(payload.content);
      onClose();
    } catch (unknownError) {
      setError(String(unknownError));
    } finally {
      setSaving(false);
    }
  }, [serverId, content, onClose]);

  // 与后端口径对齐：按 Unicode 标量计数（content.length 是 UTF-16 code unit，
  // emoji 等非 BMP 字符会按 2 计而提前误报）。
  const charCount = [...content].length;
  const overLimit = charCount > MEMO_MAX_CHARS;

  return (
    <DialogPrimitive.Root
      open
      onOpenChange={(next) => {
        // 保存进行中禁止 ESC / 点击遮罩关闭：保存请求仍在后台执行，
        // 中途关闭会丢失成功/失败反馈，让「取消」看起来像未保存。
        if (!next && !saving) onClose();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-set-confirm-overlay" />
        <DialogPrimitive.Content
          className="ai-set-confirm is-wide"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogPrimitive.Title className="ai-set-confirm-title">
            运维备忘录 · {serverLabel || serverId}
          </DialogPrimitive.Title>
          <DialogPrimitive.Description className="ai-set-confirm-description">
            智能体在运维这台服务器时读取并更新此备忘录：记录部署/服务路径、特殊命令与操作方式、已知问题与解法等长期有效信息。全文上限{" "}
            {MEMO_MAX_CHARS} 字符，请保持精简；禁止记录密码、密钥等凭据。
          </DialogPrimitive.Description>

          {loading ? (
            <div className="py-6 text-center text-[12px] text-[var(--text-muted)]">
              正在加载备忘录…
            </div>
          ) : (
            <div className="flex flex-col gap-1 py-1">
              <Textarea
                value={content}
                onChange={(event) => setContent(event.target.value)}
                rows={16}
                spellCheck={false}
                className="resize-y font-mono text-[12px] leading-relaxed"
                placeholder={
                  "## 部署路径\n\n- /opt/app：应用代码\n- /etc/app/app.conf：配置\n\n## 特殊命令与操作方式\n\n- 重启服务：systemctl restart app --legacy\n\n## 已知问题与解法"
                }
              />
              <div
                className={
                  overLimit
                    ? "text-[11px] text-[var(--danger)]"
                    : "text-[11px] text-[var(--text-muted)]"
                }
              >
                {charCount} / {MEMO_MAX_CHARS} 字符
                {overLimit ? "（超出上限，保存会被拒绝）" : ""} · 保存后清空内容视为删除备忘录
              </div>
              {error && <p className="ai-set-field-error">{error}</p>}
            </div>
          )}

          <div className="ai-set-confirm-actions">
            <Button variant="outline" size="sm" disabled={saving} onClick={onClose}>
              取消
            </Button>
            <Button size="sm" disabled={loading || saving || overLimit} onClick={save}>
              {saving ? "保存中…" : "保存"}
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
