/**
 * 普通聊天文件侧栏的「位置」分区：共享工程目录与会话临时目录同构为
 * 紧凑单行列表（图标 + 名称 + 行尾 muted 父目录），主名称优先占位、父目录
 * 让位截断，极窄宽度下由容器查询整段隐藏（见 tailwind.css 的 @container）。
 * 会话临时目录的会话切换器内嵌在该行右端（chevron），不再通栏 Select。
 * 纯呈现组件：选择逻辑与数据推导都在布局/模型层。
 */

import { AlertTriangle, ChevronDown, Folder, FolderClock } from "lucide-react";
import type { ChatSession } from "../../types";
import { cn } from "../../lib/cn";
import { formatRelativeTime } from "../../utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { pathBaseName, splitPathDisplay, type ChatFileRoot } from "./chat-files-model";

export interface ChatFilesDirectoryNavProps {
  roots: ChatFileRoot[];
  selectedRoot: string | null;
  onSelectRoot: (path: string) => void;
  sessions: ChatSession[];
  selectedSessionId: string | null;
  onSelectSession: (sessionId: string) => void;
  sessionDirExists: boolean;
  /** 总览请求进行中（位置列表先占位，避免出现「空目录」的误导）。 */
  loading: boolean;
  /** 推断出的 home 前缀，用于把绝对路径缩成 `~/…`（未知时原样返回）。 */
  homeDir: string | null;
}

const sessionLabel = (session: ChatSession | null) =>
  session ? session.title.trim() || "新对话" : "当前会话";

/** 行尾 muted 父目录：常显（便于区分同名目录），极窄宽度由 CSS 容器查询隐藏。 */
function LocParent({ parent, fullPath }: { parent: string; fullPath: string }) {
  if (!parent) {
    return null;
  }
  return (
    <span className="ai-chat-files-loc-parent" title={fullPath}>
      {parent}
    </span>
  );
}

export function ChatFilesDirectoryNav({
  roots,
  selectedRoot,
  onSelectRoot,
  sessions,
  selectedSessionId,
  onSelectSession,
  sessionDirExists,
  loading,
  homeDir,
}: ChatFilesDirectoryNavProps) {
  const projectRoots = roots.filter((root) => root.kind === "project");
  const sessionRoot = roots.find((root) => root.kind === "session") ?? null;
  const selectedSession = sessions.find((session) => session.id === selectedSessionId) ?? null;
  const sessionName = sessionLabel(selectedSession);
  const sessionActive = sessionRoot !== null && selectedRoot === sessionRoot.path;
  const hasContent = roots.length > 0 || sessions.length > 0;
  // 父目录按 root.key 预算：保留为主名称的补充信息，不改动选择逻辑。
  const locParents = Object.fromEntries(
    projectRoots.map((root) => [
      root.key,
      splitPathDisplay(root.path, { homeDir, maxChars: 22 }).parent,
    ]),
  ) as Record<string, string>;

  return (
    <nav className="ai-chat-files-locations chat-scroll" aria-label="浏览位置">
      <div className="ai-chat-files-locations-head">
        <span className="ai-chat-files-section-label">位置</span>
        {roots.length > 0 && <span className="ai-chat-files-section-count">{roots.length}</span>}
      </div>

      {loading ? (
        <p className="ai-chat-files-loc-empty">正在读取目录…</p>
      ) : !hasContent ? (
        <p className="ai-chat-files-loc-empty">
          暂无可用目录；可在 设置 · 工具 · 聊天分类 中绑定共享工程目录。
        </p>
      ) : (
        <ul className="ai-chat-files-loc-list">
          {projectRoots.map((root) => {
            const invalid = root.error !== null;
            const active = !invalid && selectedRoot === root.path;
            return (
              <li
                key={root.key}
                className={cn(
                  "ai-chat-files-loc-row",
                  invalid && "is-invalid",
                  active && "is-active",
                )}
              >
                <button
                  type="button"
                  className="ai-chat-files-loc-main"
                  // 失效目录保持可 hover（原生 title 在 disabled 上不弹），点击侧 guard 掉。
                  aria-disabled={invalid || undefined}
                  aria-current={active || undefined}
                  title={invalid ? `${root.path}\n${root.error}` : root.path}
                  onClick={() => {
                    if (!invalid) onSelectRoot(root.path);
                  }}
                >
                  {invalid ? (
                    <AlertTriangle
                      className="ai-chat-files-loc-icon"
                      size={14}
                      aria-hidden="true"
                    />
                  ) : (
                    <Folder className="ai-chat-files-loc-icon" size={14} aria-hidden="true" />
                  )}
                  <span className="ai-chat-files-loc-name">
                    {pathBaseName(root.path) || root.path}
                  </span>
                  {invalid ? (
                    <span className="ai-chat-files-loc-meta is-warning">失效</span>
                  ) : (
                    <LocParent parent={locParents[root.key] ?? ""} fullPath={root.path} />
                  )}
                </button>
              </li>
            );
          })}

          {(sessionRoot || sessions.length > 0) && (
            <li className="ai-chat-files-loc-sep" role="presentation" aria-hidden="true" />
          )}

          {(sessionRoot || sessions.length > 0) && (
            <li className={cn("ai-chat-files-loc-row", sessionActive && "is-active")}>
              {sessionRoot ? (
                <button
                  type="button"
                  className="ai-chat-files-loc-main"
                  aria-current={sessionActive || undefined}
                  title={`会话临时目录 · ${sessionRoot.path}`}
                  onClick={() => onSelectRoot(sessionRoot.path)}
                >
                  <FolderClock className="ai-chat-files-loc-icon" size={14} aria-hidden="true" />
                  <span className="ai-chat-files-loc-name">{sessionName}</span>
                  {!sessionDirExists && (
                    <span className="ai-chat-files-loc-meta is-pending">
                      <span className="ai-chat-files-loc-dot" aria-hidden="true" />
                      未创建
                    </span>
                  )}
                </button>
              ) : (
                <span className="ai-chat-files-loc-main is-static">
                  <FolderClock className="ai-chat-files-loc-icon" size={14} aria-hidden="true" />
                  <span className="ai-chat-files-loc-name">{sessionName}</span>
                </span>
              )}

              {sessions.length > 0 && (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      className="ai-chat-files-loc-switch"
                      title="切换会话"
                      aria-label="切换会话临时目录"
                    >
                      <ChevronDown size={13} aria-hidden="true" />
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent
                    align="end"
                    className="ai-context-menu ai-chat-files-session-menu"
                  >
                    {/* 会话可能很多：内层容器滚动（外层 DropdownMenuContent 自带
                        overflow-hidden 工具类，覆盖不到它）。 */}
                    <div className="chat-scroll max-h-[min(60vh,320px)] overflow-y-auto">
                      {sessions.map((session) => (
                        <DropdownMenuItem
                          key={session.id}
                          className={cn(
                            "ai-context-menu-item",
                            "ai-chat-files-session-menu-item",
                            session.id === selectedSessionId && "is-current",
                          )}
                          aria-current={session.id === selectedSessionId || undefined}
                          onSelect={() => onSelectSession(session.id)}
                        >
                          <span className="ai-chat-files-session-menu-title">
                            {sessionLabel(session)}
                          </span>
                          <span className="ai-chat-files-session-time">
                            {formatRelativeTime(session.updatedAt)}
                          </span>
                        </DropdownMenuItem>
                      ))}
                    </div>
                  </DropdownMenuContent>
                </DropdownMenu>
              )}
            </li>
          )}
        </ul>
      )}
    </nav>
  );
}
