import { useEffect } from "react";
import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ChatSession,
  DispatcherSession,
  ProjectSession,
  SessionKeyword,
  SessionPage,
  SessionSearchResult,
} from "../types";
import { ARCH_DESIGN_CATEGORY } from "../types/architecture";
import {
  withDispatcherSessionRunning,
  withDispatcherSessionsRunning,
} from "../components/dispatcherSessionStore";

/**
 * 会话列表数据层（聊天 / 项目两侧共用）。
 *
 * 两侧差异仅在 query key 前缀、列表命令与缓存形状（聊天侧单页扁平数组、
 * 项目侧 offset 无限分页）；事件合并（dispatcher-session-updated /
 * session-keywords-updated → setQueryData 乐观合并 + invalidate）收敛在
 * useSessionListEventMerge 单一实现中，按 SessionListScope 过滤与写入。
 */

export type SessionListScope =
  | { kind: "chat" }
  | { kind: "project"; projectId: string };

export const SESSION_QUERY_KEYS = {
  // 与 use-chat-queries.ts 中聊天列表的既有 key 完全一致（["chat","sessions","all"]）。
  chatList: ["chat", "sessions", "all"] as const,
  projectList: (projectId: string) => ["project", "sessions", projectId] as const,
  sessionSearch: (query: string, kind: string, projectId?: string | null) =>
    ["sessions", "search", kind, projectId ?? "all", query] as const,
} as const;

export type SessionPages<T> = InfiniteData<SessionPage<T>>;

type SessionListItem = {
  id: string;
  updatedAt: string;
  keywords?: string[];
};

// ── 纯函数：列表合并 ────────────────────────────────────────────────────────

function sortSessionsByUpdatedAt<T extends { updatedAt: string }>(sessions: T[]): T[] {
  return [...sessions].sort((left, right) => {
    const leftTime = Date.parse(left.updatedAt);
    const rightTime = Date.parse(right.updatedAt);
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });
}

/** 按 id 合并单条会话增量：已存在则原位更新（keywords 缺失时保留旧值），否则前插，整体按 updatedAt 重排。 */
function mergeSessionIntoList<T extends SessionListItem>(
  sessions: T[] | undefined,
  updated: Omit<T, "keywords"> & { keywords?: string[] },
): T[] | undefined {
  if (!sessions) return sessions;
  const existing = sessions.find((session) => session.id === updated.id);
  const normalized = {
    ...updated,
    keywords: updated.keywords ?? existing?.keywords ?? [],
  } as T;
  const next = existing
    ? sessions.map((session) => (session.id === normalized.id ? normalized : session))
    : [normalized, ...sessions];
  return sortSessionsByUpdatedAt(next);
}

function mergeKeywordsIntoList<T extends SessionListItem>(
  sessions: T[] | undefined,
  sessionId: string,
  keywords: string[],
): T[] | undefined {
  return sessions?.map((session) =>
    session.id === sessionId ? { ...session, keywords } : session,
  );
}

/** infinite 分页缓存上的同款合并：扁平化 → 复用单列表合并 → 按原分页大小重新切块。 */
function mergeSessionIntoPages<T extends SessionListItem>(
  data: SessionPages<T> | undefined,
  updated: Omit<T, "keywords"> & { keywords?: string[] },
): SessionPages<T> | undefined {
  if (!data) return data;
  const items = data.pages.flatMap((page) => page.items);
  const exists = items.some((session) => session.id === updated.id);
  const merged = mergeSessionIntoList(items, updated);
  if (!merged) return data;
  let offset = 0;
  const pages = data.pages.map((page, index) => {
    const size = page.items.length + (index === 0 && !exists ? 1 : 0);
    const slice = merged.slice(offset, offset + size);
    offset += size;
    return { ...page, items: slice, total: exists ? page.total : page.total + 1 };
  });
  return { ...data, pages };
}

function mergeKeywordsIntoPages<T extends SessionListItem>(
  data: SessionPages<T> | undefined,
  sessionId: string,
  keywords: string[],
): SessionPages<T> | undefined {
  if (!data) return data;
  return {
    ...data,
    pages: data.pages.map((page) => ({
      ...page,
      items: mergeKeywordsIntoList(page.items, sessionId, keywords) ?? page.items,
    })),
  };
}

function removeSessionFromPages<T extends SessionListItem>(
  data: SessionPages<T> | undefined,
  sessionId: string,
): SessionPages<T> | undefined {
  if (!data) return data;
  let removed = false;
  const pages = data.pages.map((page) => {
    const items = page.items.filter((session) => session.id !== sessionId);
    if (items.length !== page.items.length) removed = true;
    return { ...page, items };
  });
  if (!removed) return data;
  const [first, ...rest] = pages;
  return {
    ...data,
    pages: first ? [{ ...first, total: Math.max(0, first.total - 1) }, ...rest] : pages,
  };
}

/** 扁平化 infinite 分页缓存（按 id 去重，保持分页顺序）。 */
export function flattenSessionPages<T extends { id: string }>(
  data: SessionPages<T> | undefined,
): T[] {
  if (!data) return [];
  const seen = new Set<string>();
  const items: T[] = [];
  for (const page of data.pages) {
    for (const item of page.items) {
      if (seen.has(item.id)) continue;
      seen.add(item.id);
      items.push(item);
    }
  }
  return items;
}

// ── 事件合并（两侧唯一实现） ────────────────────────────────────────────────

/**
 * 事件到 DB 回源查询的合并窗口：agent 运行期间 dispatcher-session-updated
 * 高频到来，逐条事件 invalidate 会让列表/搜索查询反复全表重查；乐观合并
 * （setQueryData）保持即时，回源失效统一在窗口末尾做一次。
 */
const INVALIDATE_DEBOUNCE_MS = 300;

export function useSessionListEventMerge(scope: SessionListScope, enabled = true) {
  const qc = useQueryClient();
  const scopeKind = scope.kind;
  const scopeProjectId = scope.kind === "project" ? scope.projectId : null;

  useEffect(() => {
    if (!enabled) return;
    if (scopeKind === "project" && !scopeProjectId) return;

    const listPrefix: readonly unknown[] =
      scopeKind === "chat"
        ? ["chat", "sessions"]
        : ["project", "sessions", scopeProjectId];

    let invalidateTimer: ReturnType<typeof setTimeout> | null = null;
    const scheduleInvalidate = () => {
      if (invalidateTimer !== null) return;
      invalidateTimer = setTimeout(() => {
        invalidateTimer = null;
        void qc.invalidateQueries({ queryKey: listPrefix });
        void qc.invalidateQueries({ queryKey: ["sessions", "search"] });
      }, INVALIDATE_DEBOUNCE_MS);
    };

    const unlistenSession = listen<DispatcherSession>("dispatcher-session-updated", (event) => {
      const payload = event.payload;
      if (scopeKind === "chat") {
        if (payload.kind !== "chat") return;
        // 内部分类（架构设计助手）会话不进主聊天列表缓存——它们由专属界面
        // 按分类自管，且后端默认列表/搜索已做同口径排除。
        if (payload.category === ARCH_DESIGN_CATEGORY) return;
        // keywords 不做默认值兜底：payload 缺省时由合并逻辑保留缓存中的旧值。
        const updatedSession = withDispatcherSessionRunning({
          id: payload.id,
          title: payload.title,
          category: payload.category,
          createdAt: payload.createdAt,
          updatedAt: payload.updatedAt,
          keywords: payload.keywords,
        });
        qc.setQueryData<ChatSession[]>(SESSION_QUERY_KEYS.chatList, (sessions) =>
          mergeSessionIntoList(sessions, updatedSession),
        );
      } else {
        if (!scopeProjectId) return;
        if (payload.kind !== "project" || payload.projectId !== scopeProjectId) return;
        const updatedSession = withDispatcherSessionRunning({
          id: payload.id,
          projectId: payload.projectId,
          title: payload.title,
          createdAt: payload.createdAt,
          updatedAt: payload.updatedAt,
          keywords: payload.keywords,
        });
        qc.setQueryData<SessionPages<ProjectSession>>(
          SESSION_QUERY_KEYS.projectList(scopeProjectId),
          (data) => mergeSessionIntoPages(data, updatedSession),
        );
      }
      scheduleInvalidate();
    });
    const unlistenKeywords = listen<{
      sessionId: string;
      keywords: SessionKeyword[];
    }>("session-keywords-updated", (event) => {
      const { sessionId, keywords } = event.payload;
      const values = keywords.map((keyword) => keyword.keyword);
      if (scopeKind === "chat") {
        qc.setQueryData<ChatSession[]>(SESSION_QUERY_KEYS.chatList, (sessions) =>
          mergeKeywordsIntoList(sessions, sessionId, values),
        );
      } else if (scopeProjectId) {
        qc.setQueryData<SessionPages<ProjectSession>>(
          SESSION_QUERY_KEYS.projectList(scopeProjectId),
          (data) => mergeKeywordsIntoPages(data, sessionId, values),
        );
      }
      scheduleInvalidate();
    });

    return () => {
      if (invalidateTimer !== null) clearTimeout(invalidateTimer);
      unlistenSession.then((stopListening) => stopListening()).catch(() => {});
      unlistenKeywords.then((stopListening) => stopListening()).catch(() => {});
    };
  }, [enabled, qc, scopeKind, scopeProjectId]);
}

// ── 搜索（两侧共用，命令本身已按 kind/projectId 参数化） ─────────────────────

export function useSessionSearchQuery({
  query,
  kind,
  projectId = null,
  limit = 20,
  enabled = true,
}: {
  query: string;
  kind: "chat" | "project";
  projectId?: string | null;
  limit?: number;
  enabled?: boolean;
}) {
  const trimmed = query.trim();
  return useQuery({
    queryKey: SESSION_QUERY_KEYS.sessionSearch(trimmed, kind, projectId),
    queryFn: () =>
      invoke<SessionSearchResult[]>("session_search_keywords", {
        query: trimmed,
        limit,
        kind,
        projectId,
      }),
    // 查询词变化 / agent 运行期失效重查时保留上一份结果，避免列表闪回空态。
    placeholderData: keepPreviousData,
    enabled: enabled && trimmed.length > 0,
  });
}

// ── 项目侧会话列表（offset 无限分页） ───────────────────────────────────────

const PROJECT_PAGE_SIZE = 30;

export function useProjectSessionsQuery(projectId: string, enabled = true) {
  return useInfiniteQuery({
    queryKey: SESSION_QUERY_KEYS.projectList(projectId),
    queryFn: async ({ pageParam }) => {
      const page = await invoke<SessionPage<ProjectSession>>("project_list_sessions", {
        projectId,
        offset: pageParam,
        pageSize: PROJECT_PAGE_SIZE,
      });
      return {
        ...page,
        items: withDispatcherSessionsRunning(page.items),
      };
    },
    initialPageParam: 0,
    getNextPageParam: (lastPage, allPages) =>
      lastPage.hasMore
        ? allPages.reduce((loaded, page) => loaded + page.items.length, 0)
        : undefined,
    enabled,
  });
}

export function useCreateProjectSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { projectId: string; title?: string }) =>
      invoke<ProjectSession>("project_create_session", {
        projectId: args.projectId,
        title: args.title ?? "新会话",
      }),
    onSuccess: (createdSession, args) => {
      const session = withDispatcherSessionRunning(createdSession);
      qc.setQueryData<SessionPages<ProjectSession>>(
        SESSION_QUERY_KEYS.projectList(args.projectId),
        (data) => {
          if (!data) return data;
          const [first, ...rest] = data.pages;
          if (!first || first.items.some((item) => item.id === session.id)) return data;
          return {
            ...data,
            pages: [
              { ...first, items: [session, ...first.items], total: first.total + 1 },
              ...rest,
            ],
          };
        },
      );
      void qc.invalidateQueries({ queryKey: ["project", "sessions", args.projectId] });
    },
  });
}

export function useDeleteProjectSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { sessionId: string; projectId: string }) =>
      invoke<void>("project_delete_session", { sessionId: args.sessionId }),
    onSuccess: (_data, args) => {
      qc.setQueryData<SessionPages<ProjectSession>>(
        SESSION_QUERY_KEYS.projectList(args.projectId),
        (data) => removeSessionFromPages(data, args.sessionId),
      );
      qc.setQueriesData<SessionSearchResult[]>(
        { queryKey: ["sessions", "search"] },
        (results) => results?.filter((result) => result.sessionId !== args.sessionId),
      );
      void qc.invalidateQueries({ queryKey: ["project", "sessions", args.projectId] });
    },
  });
}
