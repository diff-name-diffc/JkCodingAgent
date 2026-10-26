/**
 * 「分类文件」面板左栏上半区：目录分区导航——共享工程目录（失效项禁选并
 * 展示原因）与会话临时目录（带会话切换下拉）。纯呈现组件，选择逻辑在面板。
 */

import { AlertTriangle, Folder, FolderClock } from "lucide-react";
import type { ChatSession } from "../../types";
import { cn } from "../../lib/cn";
import { formatRelativeTime } from "../../utils";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";
import { pathBaseName, type ChatFileRoot } from "./chat-files-model";

export interface ChatFilesDirectoryNavProps {
  roots: ChatFileRoot[];
  selectedRoot: string | null;
  onSelectRoot: (path: string) => void;
  sessions: ChatSession[];
  selectedSessionId: string | null;
  onSelectSession: (sessionId: string) => void;
  sessionDirExists: boolean;
}

export function ChatFilesDirectoryNav({
  roots,
  selectedRoot,
  onSelectRoot,
  sessions,
  selectedSessionId,
  onSelectSession,
  sessionDirExists,
}: ChatFilesDirectoryNavProps) {
  const projectRoots = roots.filter((root) => root.kind === "project");
  const sessionRoot = roots.find((root) => root.kind === "session") ?? null;
  const selectedSession = sessions.find((session) => session.id === selectedSessionId) ?? null;

  return (
    <div className="ai-chat-files-nav chat-scroll">
      <section aria-label="共享工程目录">
        <div className="ai-chat-files-section-label">
          <Folder size={12} aria-hidden="true" />
          <span>共享工程目录</span>
          <span className="ai-chat-files-section-count">{projectRoots.length}</span>
        </div>
        {projectRoots.length === 0 ? (
          <p className="ai-chat-files-hint">
            未绑定共享工程目录；可在 设置 · 工具 · 聊天分类 中绑定。
          </p>
        ) : (
          <ul>
            {projectRoots.map((root) => {
              const invalid = root.error !== null;
              const active = selectedRoot === root.path;
              return (
                <li key={root.key}>
                  <button
                    type="button"
                    disabled={invalid}
                    className={cn(
                      "ai-chat-files-dir-item",
                      invalid && "is-invalid",
                      active && !invalid && "is-active",
                    )}
                    title={invalid ? `${root.path}\n${root.error}` : root.path}
                    onClick={() => onSelectRoot(root.path)}
                  >
                    {invalid ? (
                      <AlertTriangle
                        className="ai-chat-files-dir-icon"
                        size={14}
                        aria-hidden="true"
                      />
                    ) : (
                      <Folder className="ai-chat-files-dir-icon" size={14} aria-hidden="true" />
                    )}
                    <span className="ai-chat-files-dir-text">
                      <span className="ai-chat-files-dir-name">
                        {pathBaseName(root.path) || root.path}
                      </span>
                      <span className="ai-chat-files-dir-path">{root.path}</span>
                      {invalid && <span className="ai-chat-files-dir-error">{root.error}</span>}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-label="会话临时目录">
        <div className="ai-chat-files-section-label">
          <FolderClock size={12} aria-hidden="true" />
          <span>会话临时目录</span>
        </div>
        {sessions.length === 0 ? (
          <p className="ai-chat-files-hint">该分类暂无会话，尚无可查看的临时目录。</p>
        ) : (
          <>
            <Select
              value={selectedSessionId ?? undefined}
              onValueChange={(value) => onSelectSession(value)}
            >
              <SelectTrigger className="ai-chat-files-session-select" aria-label="选择会话">
                <SelectValue placeholder="选择会话" />
              </SelectTrigger>
              <SelectContent className="ai-context-menu">
                {sessions.map((session) => (
                  <SelectItem key={session.id} value={session.id} className="ai-context-menu-item">
                    {session.title.trim() || "新对话"}
                    <span className="ai-chat-files-session-time">
                      {formatRelativeTime(session.updatedAt)}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {sessionRoot && (
              <button
                type="button"
                className={cn(
                  "ai-chat-files-dir-item",
                  selectedRoot === sessionRoot.path && "is-active",
                )}
                title={sessionRoot.path}
                onClick={() => onSelectRoot(sessionRoot.path)}
              >
                <FolderClock className="ai-chat-files-dir-icon" size={14} aria-hidden="true" />
                <span className="ai-chat-files-dir-text">
                  <span className="ai-chat-files-dir-name">
                    {selectedSession ? selectedSession.title.trim() || "新对话" : "当前会话"}
                  </span>
                  <span className="ai-chat-files-dir-path">{sessionRoot.path}</span>
                  <span
                    className={cn(
                      "ai-chat-files-dir-badge",
                      sessionDirExists ? "is-ready" : "is-pending",
                    )}
                  >
                    {sessionDirExists ? "已创建" : "未创建"}
                  </span>
                </span>
              </button>
            )}
          </>
        )}
      </section>
    </div>
  );
}
