/**
 * 「分类文件」面板的目录选择纯逻辑：默认会话推导、可选根列表构造与
 * 默认根推导。组件层只做状态持有与渲染，便于单测。
 */
import type { ChatSession, ChatWorkspaceOverview } from "../../types";

/** 面板可选的浏览根：分类工程目录（可能失效）或会话临时目录。 */
export interface ChatFileRoot {
  key: string;
  kind: "project" | "session";
  path: string;
  /** 工程目录失效（不可浏览）时的原因；会话目录恒可选中（未创建仅提示）。 */
  error: string | null;
}

/** 默认临时目录会话：优先当前活跃会话（若属于该分类），否则取列表首个。 */
export function pickDefaultSessionId(
  activeSessionId: string | null,
  sessions: Pick<ChatSession, "id">[],
): string | null {
  if (activeSessionId && sessions.some((session) => session.id === activeSessionId)) {
    return activeSessionId;
  }
  return sessions[0]?.id ?? null;
}

/** 路径最后一段（工程目录行的主标签）；根路径或空串回退整串。 */
export function pathBaseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const separatorIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return separatorIndex === -1 ? trimmed : trimmed.slice(separatorIndex + 1);
}

/** 由总览构造可选择的根列表：工程目录在前（失效项保留供展示原因），会话目录殿后。 */
export function buildChatFileRoots(overview: ChatWorkspaceOverview | undefined): ChatFileRoot[] {
  if (!overview) {
    return [];
  }
  const roots: ChatFileRoot[] = overview.projectDirs.map((dir) => ({
    key: `project:${dir.raw}`,
    kind: "project" as const,
    // 失效目录不可浏览，但保留 raw 供展示；path 仅在有效时被选中。
    path: dir.canonical ?? dir.raw,
    error: dir.error,
  }));
  if (overview.sessionDir) {
    roots.push({
      key: "session",
      kind: "session",
      path: overview.sessionDir,
      error: null,
    });
  }
  return roots;
}

/** 默认浏览根：首个有效工程目录；都没有则取会话临时目录。 */
export function pickInitialRoot(roots: ChatFileRoot[]): string | null {
  return roots.find((root) => !root.error)?.path ?? null;
}
