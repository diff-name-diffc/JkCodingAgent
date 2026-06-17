import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MessageCircle, MonitorDot, Plus, Search, Sparkles } from "lucide-react";
import type { ChatCategory, ChatSession, SessionPage, SessionSearchResult } from "../types";
import {
  cleanupDispatcherSession,
  withDispatcherSessionRunning,
  withDispatcherSessionsRunning,
} from "./dispatcherSessionStore";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import { ChatCategorySection } from "./ChatCategorySection";
import { ChatNewCategoryDialog } from "./ChatNewCategoryDialog";
import { BrandButton } from "./ui/chatPrimitives";
import s from "../styles";

const EXPANDED_STORAGE_KEY = "nezha.chat.expandedCategories";
const DEFAULT_CHAT_CATEGORY = "tech";
const UNCATEGORIZED_CATEGORY = "__uncategorized__";
const INITIAL_CHAT_PAGE_SIZE = 50;
const LOAD_MORE_PAGE_SIZE = 10;

interface CategorySessionState {
  items: ChatSession[];
  total: number;
  hasMore: boolean;
  nextCursor: string | null;
  loading: boolean;
  loaded: boolean;
}

function sortSessionsByUpdatedAt(sessions: ChatSession[]) {
  return [...sessions].sort((left, right) => {
    const leftTime = Date.parse(left.updatedAt);
    const rightTime = Date.parse(right.updatedAt);
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });
}

function categoryKey(category: string | null | undefined) {
  return category || UNCATEGORIZED_CATEGORY;
}

function categoryParam(key: string) {
  return key === UNCATEGORIZED_CATEGORY ? "" : key;
}

function emptyCategoryState(total = 0): CategorySessionState {
  return {
    items: [],
    total,
    hasMore: false,
    nextCursor: null,
    loading: false,
    loaded: false,
  };
}

function loadExpandedCategory(): string | null {
  try {
    const raw = localStorage.getItem(EXPANDED_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed === "string") return parsed;
    if (Array.isArray(parsed) && typeof parsed[0] === "string") return parsed[0];
    return null;
  } catch {
    return null;
  }
}

function saveExpandedCategory(categoryId: string | null) {
  try {
    if (categoryId) {
      localStorage.setItem(EXPANDED_STORAGE_KEY, JSON.stringify(categoryId));
    } else {
      localStorage.removeItem(EXPANDED_STORAGE_KEY);
    }
  } catch {
    // localStorage 不可用时不影响主流程。
  }
}

function categoryExists(categoryId: string, categories: ChatCategory[], uncategorizedTotal: number) {
  if (categoryId === UNCATEGORIZED_CATEGORY) return uncategorizedTotal > 0;
  return categories.some((cat) => cat.id === categoryId);
}

function chooseInitialCategory(
  categories: ChatCategory[],
  uncategorizedTotal: number,
  savedCategoryId: string | null,
) {
  if (savedCategoryId && categoryExists(savedCategoryId, categories, uncategorizedTotal)) {
    return savedCategoryId;
  }

  const defaultCategory = categories.find((cat) => cat.id === DEFAULT_CHAT_CATEGORY);
  if (defaultCategory && defaultCategory.sessionCount > 0) return defaultCategory.id;

  const populatedCategory = categories.find((cat) => cat.sessionCount > 0);
  if (populatedCategory) return populatedCategory.id;

  if (uncategorizedTotal > 0) return UNCATEGORIZED_CATEGORY;
  return defaultCategory?.id ?? categories[0]?.id ?? DEFAULT_CHAT_CATEGORY;
}

interface ChatSessionSidebarProps {
  activeSessionId: string | null;
  onActiveSessionChange: (id: string | null) => void;
  showBrowserButton: boolean;
  onToggleBrowser: () => void;
}

export function ChatSessionSidebar({
  activeSessionId,
  onActiveSessionChange,
  showBrowserButton,
  onToggleBrowser,
}: ChatSessionSidebarProps) {
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SessionSearchResult[] | null>(null);
  const [categorySessions, setCategorySessions] = useState<Record<string, CategorySessionState>>({});
  const [categories, setCategories] = useState<ChatCategory[]>([]);
  const [uncategorizedTotal, setUncategorizedTotal] = useState(0);
  const [expandedCategoryId, setExpandedCategoryId] = useState<string | null>(
    () => loadExpandedCategory() ?? DEFAULT_CHAT_CATEGORY,
  );
  const [showNewCategoryDialog, setShowNewCategoryDialog] = useState(false);
  const [editingCategory, setEditingCategory] = useState<ChatCategory | null>(null);
  const [dragOverCategoryId, setDragOverCategoryId] = useState<string | null>(null);
  const draggedSessionIdRef = useRef<string | null>(null);
  const creatingSessionRef = useRef(false);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const activeSessionIdRef = useRef(activeSessionId);
  const categorySessionsRef = useRef(categorySessions);
  const loadingCategoriesRef = useRef(new Set<string>());
  activeSessionIdRef.current = activeSessionId;
  categorySessionsRef.current = categorySessions;

  const setCategoryTotal = useCallback((catId: string, total: number) => {
    if (catId === UNCATEGORIZED_CATEGORY) {
      setUncategorizedTotal(total);
      return;
    }
    setCategories((prev) =>
      prev.map((cat) => (cat.id === catId ? { ...cat, sessionCount: total } : cat)),
    );
  }, []);

  const adjustCategoryTotal = useCallback((catId: string, delta: number) => {
    if (catId === UNCATEGORIZED_CATEGORY) {
      setUncategorizedTotal((prev) => Math.max(0, prev + delta));
      return;
    }
    setCategories((prev) =>
      prev.map((cat) =>
        cat.id === catId ? { ...cat, sessionCount: Math.max(0, cat.sessionCount + delta) } : cat,
      ),
    );
  }, []);

  const loadCategory = useCallback(
    async (catId: string, mode: "reset" | "append" = "reset") => {
      const current = categorySessionsRef.current[catId] ?? emptyCategoryState();
      const cursor = mode === "append" ? current.nextCursor : null;
      const pageSize = mode === "append" ? LOAD_MORE_PAGE_SIZE : INITIAL_CHAT_PAGE_SIZE;

      if (loadingCategoriesRef.current.has(catId)) return current.items;
      if (mode === "append" && (!current.hasMore || !cursor)) return current.items;

      loadingCategoriesRef.current.add(catId);
      setCategorySessions((prev) => ({
        ...prev,
        [catId]: { ...(prev[catId] ?? emptyCategoryState()), loading: true },
      }));

      try {
        const page = await invoke<SessionPage<ChatSession>>("chat_list_sessions", {
          category: categoryParam(catId),
          cursor,
          pageSize,
        });
        const items = withDispatcherSessionsRunning(page.items);
        setCategoryTotal(catId, page.total);
        const previous = categorySessionsRef.current[catId] ?? emptyCategoryState(page.total);
        const mergedItems =
          mode === "append"
            ? sortSessionsByUpdatedAt([
                ...previous.items,
                ...items.filter((item) => !previous.items.some((old) => old.id === item.id)),
              ])
            : sortSessionsByUpdatedAt(items);

        setCategorySessions((prev) => {
          return {
            ...prev,
            [catId]: {
              items: mergedItems,
              total: page.total,
              hasMore: page.hasMore,
              nextCursor: page.nextCursor ?? null,
              loading: false,
              loaded: true,
            },
          };
        });
        return mergedItems;
      } catch (err) {
        setCategorySessions((prev) => ({
          ...prev,
          [catId]: { ...(prev[catId] ?? emptyCategoryState()), loading: false },
        }));
        throw err;
      } finally {
        loadingCategoriesRef.current.delete(catId);
      }
    },
    [setCategoryTotal],
  );

  const createSessionInCategory = useCallback(
    async (catId: string) => {
      const session = withDispatcherSessionRunning(
        await invoke<ChatSession>("chat_create_session", {
          title: "新聊天",
          category: categoryParam(catId) || DEFAULT_CHAT_CATEGORY,
        }),
      );
      const targetKey = categoryKey(session.category);
      adjustCategoryTotal(targetKey, 1);
      setCategorySessions((prev) => {
        const previous = prev[targetKey] ?? emptyCategoryState();
        if (!previous.loaded) return prev;
        return {
          ...prev,
          [targetKey]: {
            ...previous,
            items: sortSessionsByUpdatedAt([session, ...previous.items.filter((s) => s.id !== session.id)]),
            total: previous.total + 1,
          },
        };
      });
      setExpandedCategoryId(targetKey);
      saveExpandedCategory(targetKey);
      onActiveSessionChange(session.id);
      return session;
    },
    [adjustCategoryTotal, onActiveSessionChange],
  );

  const loadInitial = useCallback(async () => {
    const [cats, uncategorizedPage] = await Promise.all([
      invoke<ChatCategory[]>("chat_list_categories"),
      invoke<SessionPage<ChatSession>>("chat_list_sessions", {
        category: "",
        cursor: null,
        pageSize: 0,
      }),
    ]);
    const uncategorizedCount = uncategorizedPage.total;
    setCategories(cats);
    setUncategorizedTotal(uncategorizedCount);

    const totalSessions = cats.reduce((sum, cat) => sum + cat.sessionCount, 0) + uncategorizedCount;
    if (totalSessions === 0) {
      const session = await createSessionInCategory(DEFAULT_CHAT_CATEGORY);
      await loadCategory(categoryKey(session.category), "reset");
      return;
    }

    const initialCategoryId = chooseInitialCategory(cats, uncategorizedCount, loadExpandedCategory());
    setExpandedCategoryId(initialCategoryId);
    saveExpandedCategory(initialCategoryId);
    const loaded = await loadCategory(initialCategoryId, "reset");

    const current = activeSessionIdRef.current;
    if (!current && loaded.length > 0) {
      onActiveSessionChange(loaded[0].id);
    } else if (current && !loaded.some((session) => session.id === current)) {
      onActiveSessionChange(loaded[0]?.id ?? current);
    }
  }, [createSessionInCategory, loadCategory, onActiveSessionChange]);

  useEffect(() => {
    let cancelled = false;
    loadInitial().catch((err) => {
      if (!cancelled) console.error("加载聊天失败:", err);
    });
    return () => {
      cancelled = true;
    };
  }, [loadInitial]);

  const expandedState = expandedCategoryId
    ? categorySessions[expandedCategoryId] ?? emptyCategoryState()
    : emptyCategoryState();

  const loadMore = useCallback(async () => {
    if (!expandedCategoryId) return;
    await loadCategory(expandedCategoryId, "append");
  }, [expandedCategoryId, loadCategory]);

  useEffect(() => {
    if (!sentinelRef.current || searchResults !== null) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && expandedState.hasMore && !expandedState.loading) {
          void loadMore().catch((err) => console.error("加载更多聊天失败:", err));
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(sentinelRef.current);
    return () => observer.disconnect();
  }, [expandedState.hasMore, expandedState.loading, loadMore, searchResults]);

  useEffect(() => {
    const unlisten = listen<ChatSession>("dispatcher-session-updated", (event) => {
      const payload = event.payload as ChatSession & { category?: unknown };
      if (typeof payload.category !== "string") return;
      const updated = withDispatcherSessionRunning(payload);
      const updatedKey = categoryKey(updated.category);
      setCategorySessions((prev) => {
        let touched = false;
        const next = Object.fromEntries(
          Object.entries(prev).map(([catId, state]) => {
            const contains = state.items.some((session) => session.id === updated.id);
            if (!contains && catId !== updatedKey) return [catId, state];
            touched = true;
            const withoutCurrent = state.items.filter((session) => session.id !== updated.id);
            const nextItems =
              catId === updatedKey ? sortSessionsByUpdatedAt([updated, ...withoutCurrent]) : withoutCurrent;
            return [catId, { ...state, items: nextItems }];
          }),
        );
        return touched ? next : prev;
      });
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<ChatCategory>("chat-category-updated", (event) => {
      const updated = event.payload;
      setCategories((prev) => prev.map((cat) => (cat.id === updated.id ? updated : cat)));
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const loadedSessions = useMemo(
    () => Object.values(categorySessions).flatMap((state) => state.items),
    [categorySessions],
  );
  const sessionIds = useMemo(() => loadedSessions.map((session) => session.id), [loadedSessions]);
  const runningSessionIds = useDispatcherSessionRunningSet(sessionIds);

  useEffect(() => {
    if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    const trimmed = query.trim();
    if (!trimmed) {
      setSearchResults(null);
      return;
    }
    searchTimerRef.current = setTimeout(() => {
      invoke<SessionSearchResult[]>("session_search_keywords", {
        query: trimmed,
        limit: 20,
        kind: "chat",
        projectId: null,
      })
        .then(setSearchResults)
        .catch(() => setSearchResults([]));
    }, 260);
    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, [query]);

  const handleCreateSession = async (categoryId?: string) => {
    if (creatingSessionRef.current) return;
    creatingSessionRef.current = true;
    try {
      const targetCategoryId = categoryId ?? DEFAULT_CHAT_CATEGORY;
      const session = await createSessionInCategory(targetCategoryId);
      await loadCategory(categoryKey(session.category), "reset");
    } catch (err) {
      console.error("创建聊天失败:", err);
    } finally {
      creatingSessionRef.current = false;
    }
  };

  const handleDeleteSession = async (sessionId: string, e: React.MouseEvent) => {
    e.stopPropagation();
    const ok = await confirm("确定永久删除这个聊天吗？", { title: "删除聊天", kind: "warning" });
    if (!ok) return;

    let deletedCategoryId: string | null = null;
    let nextActiveSessionId: string | null = null;
    for (const [catId, state] of Object.entries(categorySessions)) {
      if (!state.items.some((session) => session.id === sessionId)) continue;
      deletedCategoryId = catId;
      nextActiveSessionId = state.items.find((session) => session.id !== sessionId)?.id ?? null;
      break;
    }

    try {
      await invoke("chat_delete_session", { sessionId });
      cleanupDispatcherSession(sessionId);
      setCategorySessions((prev) => {
        const next = { ...prev };
        for (const [catId, state] of Object.entries(prev)) {
          if (!state.items.some((session) => session.id === sessionId)) continue;
          const items = state.items.filter((session) => session.id !== sessionId);
          next[catId] = {
            ...state,
            items,
            total: Math.max(0, state.total - 1),
          };
        }
        return next;
      });

      if (deletedCategoryId) adjustCategoryTotal(deletedCategoryId, -1);
      if (activeSessionId === sessionId) onActiveSessionChange(nextActiveSessionId);
      if (allKnownSessionCount <= 1) {
        const session = await createSessionInCategory(DEFAULT_CHAT_CATEGORY);
        await loadCategory(categoryKey(session.category), "reset");
      }
    } catch (err) {
      console.error("删除聊天失败:", err);
    }
  };

  const expandCategory = (catId: string) => {
    setExpandedCategoryId(catId);
    saveExpandedCategory(catId);
    const state = categorySessions[catId];
    if (!state?.loaded) {
      void loadCategory(catId, "reset").catch((err) => console.error("加载分类聊天失败:", err));
    }
  };

  const handleCategoryDragStart = (sessionId: string, e: React.DragEvent) => {
    draggedSessionIdRef.current = sessionId;
    e.dataTransfer.setData("text/plain", sessionId);
    e.dataTransfer.effectAllowed = "move";
  };

  const handleCategoryDragOver = (_e: React.DragEvent) => {
    // Allow drop.
  };

  const handleCategoryDrop = async (categoryId: string, e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const sessionId = e.dataTransfer.getData("text/plain") || draggedSessionIdRef.current;
    setDragOverCategoryId(null);
    draggedSessionIdRef.current = null;
    if (!sessionId) return;

    let movedSession: ChatSession | null = null;
    let sourceCategoryId: string | null = null;
    for (const [catId, state] of Object.entries(categorySessions)) {
      const found = state.items.find((session) => session.id === sessionId);
      if (found) {
        movedSession = found;
        sourceCategoryId = catId;
        break;
      }
    }
    if (sourceCategoryId === categoryId) return;

    try {
      await invoke("chat_set_session_category_v6", {
        sessionId,
        categoryId: categoryParam(categoryId),
      });

      if (sourceCategoryId) adjustCategoryTotal(sourceCategoryId, -1);
      adjustCategoryTotal(categoryId, 1);

      setCategorySessions((prev) => {
        const next = { ...prev };
        if (sourceCategoryId && next[sourceCategoryId]) {
          next[sourceCategoryId] = {
            ...next[sourceCategoryId],
            items: next[sourceCategoryId].items.filter((session) => session.id !== sessionId),
            total: Math.max(0, next[sourceCategoryId].total - 1),
          };
        }
        if (movedSession && next[categoryId]?.loaded) {
          const updatedSession = {
            ...movedSession,
            category: categoryParam(categoryId),
          };
          next[categoryId] = {
            ...next[categoryId],
            items: sortSessionsByUpdatedAt([
              updatedSession,
              ...next[categoryId].items.filter((session) => session.id !== sessionId),
            ]),
            total: next[categoryId].total + 1,
          };
        }
        return next;
      });

      expandCategory(categoryId);
    } catch (err) {
      console.error("移动会话失败:", err);
    }
  };

  const handleCreateCategory = async (name: string) => {
    try {
      const cat = await invoke<ChatCategory>("chat_create_category", { name });
      setCategories((prev) => [...prev, cat]);
      setShowNewCategoryDialog(false);
    } catch (err) {
      console.error("创建分类失败:", err);
    }
  };

  const handleRenameCategory = (cat: ChatCategory) => {
    setEditingCategory(cat);
  };

  const handleUpdateCategory = async (name: string) => {
    if (!editingCategory) return;
    try {
      const updated = await invoke<ChatCategory | null>("chat_update_category", {
        categoryId: editingCategory.id,
        name,
      });
      if (updated) {
        setCategories((prev) => prev.map((cat) => (cat.id === updated.id ? updated : cat)));
      }
      setEditingCategory(null);
    } catch (err) {
      console.error("更新分类失败:", err);
    }
  };

  const handleDeleteCategory = async (cat: ChatCategory) => {
    const ok = await confirm(`确定删除分类"${cat.name}"吗？该分类下的会话将移至未分类。`, {
      title: "删除分类",
      kind: "warning",
    });
    if (!ok) return;
    try {
      await invoke("chat_delete_category", { categoryId: cat.id });
      const movedCount = cat.sessionCount;
      setCategories((prev) => prev.filter((item) => item.id !== cat.id));
      setUncategorizedTotal((prev) => prev + movedCount);
      setCategorySessions((prev) => {
        const removed = prev[cat.id];
        const next = { ...prev };
        delete next[cat.id];
        if (removed?.items.length) {
          const uncategorized = next[UNCATEGORIZED_CATEGORY] ?? emptyCategoryState();
          next[UNCATEGORIZED_CATEGORY] = {
            ...uncategorized,
            items: sortSessionsByUpdatedAt([
              ...removed.items.map((session) => ({ ...session, category: "" })),
              ...uncategorized.items,
            ]),
            total: uncategorized.total + movedCount,
            loaded: uncategorized.loaded,
          };
        }
        return next;
      });
      if (expandedCategoryId === cat.id) expandCategory(UNCATEGORIZED_CATEGORY);
    } catch (err) {
      console.error("删除分类失败:", err);
    }
  };

  const handleSearchResultClick = (result: SessionSearchResult) => {
    const catId = categoryKey(result.category);
    setExpandedCategoryId(catId);
    saveExpandedCategory(catId);
    if (!categorySessions[catId]?.loaded) {
      void loadCategory(catId, "reset").catch((err) => console.error("加载搜索结果分类失败:", err));
    }
    onActiveSessionChange(result.sessionId);
  };

  const sortedCategories = useMemo(
    () => [...categories].sort((a, b) => a.sortOrder - b.sortOrder),
    [categories],
  );

  const allKnownSessionCount = useMemo(
    () => categories.reduce((sum, cat) => sum + cat.sessionCount, 0) + uncategorizedTotal,
    [categories, uncategorizedTotal],
  );

  return (
    <div style={s.chatSessionPanel}>
      <div style={s.chatSidebarHero}>
        <div style={s.chatSidebarHeroTop}>
          <div style={s.chatPanelIcon}>
            <MessageCircle size={17} strokeWidth={2.6} />
          </div>
          <div style={s.chatSidebarTitleBlock as React.CSSProperties}>
            <span style={s.chatSidebarTitle}>Nezha Chat</span>
            <span style={s.chatSidebarSubtitle}>AI 编程会话工作台</span>
          </div>
          <Sparkles
            size={15}
            strokeWidth={2.2}
            style={{ marginLeft: "auto", color: "var(--warning)", flexShrink: 0 }}
          />
        </div>

        <div style={s.chatSidebarSearch}>
          <Search size={14} strokeWidth={2.2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
          <input
            style={s.panelSearchInput}
            placeholder="搜索标题或关键词"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </div>

      <div style={s.chatSidebarActions}>
        <BrandButton
          className="chat-sidebar-action"
          variant="soft"
          color="mint"
          highContrast
          style={s.chatNewSessionBtn}
          onClick={() => handleCreateSession()}
        >
          <Plus size={14} strokeWidth={2.5} />
          新建聊天
        </BrandButton>
        {showBrowserButton && (
          <BrandButton
            className="chat-sidebar-action"
            variant="surface"
            color="gray"
            style={s.chatNewSessionBtn}
            onClick={onToggleBrowser}
            title="CloakBrowser"
          >
            <MonitorDot size={14} strokeWidth={2.5} />
            浏览器
          </BrandButton>
        )}
      </div>

      <div style={s.taskDivider} />

      <div style={s.taskListScroll as React.CSSProperties}>
        {searchResults !== null ? (
          searchResults.length === 0 ? (
            <div style={s.taskListEmpty}>没有找到匹配的聊天</div>
          ) : (
            searchResults.map((result) => (
              <button
                key={result.sessionId}
                type="button"
                className={[
                  "chat-session-card",
                  result.sessionId === activeSessionId ? "is-active" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                style={{
                  ...s.sessionCard,
                  textAlign: "left" as const,
                  cursor: "pointer",
                  width: "calc(100% - 16px)",
                }}
                onClick={() => handleSearchResultClick(result)}
              >
                <div style={s.sessionCardBody as React.CSSProperties}>
                  <div style={s.sessionCardTitle}>{result.sessionTitle}</div>
                  {result.matchedKeywords.length > 0 && (
                    <div style={s.matchedKeywordsRow}>
                      {result.matchedKeywords.slice(0, 4).map((keyword) => (
                        <span key={keyword} style={s.matchedKeywordTag}>
                          {keyword}
                        </span>
                      ))}
                      {result.matchedKeywords.length > 4 && (
                        <span style={{ ...s.matchedKeywordTag, opacity: 0.6 }}>
                          +{result.matchedKeywords.length - 4}
                        </span>
                      )}
                    </div>
                  )}
                </div>
              </button>
            ))
          )
        ) : (
          <>
            {sortedCategories.map((cat) => {
              const catId = cat.id;
              const state = categorySessions[catId] ?? emptyCategoryState(cat.sessionCount);
              return (
                <div key={catId}>
                  <ChatCategorySection
                    category={cat}
                    sessions={state.items}
                    totalSessions={cat.sessionCount}
                    activeSessionId={activeSessionId}
                    runningSessionIds={runningSessionIds}
                    isExpanded={expandedCategoryId === catId}
                    onToggle={() => expandCategory(catId)}
                    onSessionClick={onActiveSessionChange}
                    onSessionDelete={handleDeleteSession}
                    onSessionDragStart={handleCategoryDragStart}
                    onNewInCategory={() => handleCreateSession(catId)}
                    onRenameCategory={() => handleRenameCategory(cat)}
                    onDeleteCategory={() => handleDeleteCategory(cat)}
                    onDragOver={(e) => {
                      e.preventDefault();
                      setDragOverCategoryId(catId);
                      handleCategoryDragOver(e);
                    }}
                    onDrop={(e) => handleCategoryDrop(catId, e)}
                    dragOverId={dragOverCategoryId}
                  />
                  {expandedCategoryId === catId && state.hasMore && (
                    <div ref={sentinelRef} style={{ height: 1, width: "100%" }} />
                  )}
                </div>
              );
            })}

            {uncategorizedTotal > 0 && (
              <div>
                <ChatCategorySection
                  category={null}
                  sessions={categorySessions[UNCATEGORIZED_CATEGORY]?.items ?? []}
                  totalSessions={uncategorizedTotal}
                  activeSessionId={activeSessionId}
                  runningSessionIds={runningSessionIds}
                  isExpanded={expandedCategoryId === UNCATEGORIZED_CATEGORY}
                  onToggle={() => expandCategory(UNCATEGORIZED_CATEGORY)}
                  onSessionClick={onActiveSessionChange}
                  onSessionDelete={handleDeleteSession}
                  onSessionDragStart={handleCategoryDragStart}
                  onNewInCategory={() => {}}
                  onRenameCategory={() => {}}
                  onDeleteCategory={() => {}}
                  onDragOver={(e) => {
                    e.preventDefault();
                    setDragOverCategoryId(UNCATEGORIZED_CATEGORY);
                  }}
                  onDrop={(e) => handleCategoryDrop(UNCATEGORIZED_CATEGORY, e)}
                  dragOverId={dragOverCategoryId}
                />
                {expandedCategoryId === UNCATEGORIZED_CATEGORY &&
                  categorySessions[UNCATEGORIZED_CATEGORY]?.hasMore && (
                    <div ref={sentinelRef} style={{ height: 1, width: "100%" }} />
                  )}
              </div>
            )}

            {allKnownSessionCount === 0 && <div style={s.taskListEmpty}>没有找到聊天</div>}
          </>
        )}
      </div>

      <div style={s.categoryFooterRow as React.CSSProperties}>
        <button style={s.categoryNewBtn} onClick={() => setShowNewCategoryDialog(true)}>
          <Plus size={12} />
          新建分类
        </button>
      </div>

      <ChatNewCategoryDialog
        open={showNewCategoryDialog}
        initialName=""
        onSubmit={handleCreateCategory}
        onClose={() => setShowNewCategoryDialog(false)}
        title="新建分类"
        confirmLabel="创建"
      />

      <ChatNewCategoryDialog
        open={editingCategory !== null}
        initialName={editingCategory?.name ?? ""}
        onSubmit={handleUpdateCategory}
        onClose={() => setEditingCategory(null)}
        title="重命名分类"
        confirmLabel="保存"
      />
    </div>
  );
}
