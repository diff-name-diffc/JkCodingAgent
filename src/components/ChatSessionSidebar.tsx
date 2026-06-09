import { useEffect, useMemo, useRef, useState, useCallback } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MessageCircle, MonitorDot, Plus, Search } from "lucide-react";
import type { ChatCategory, ChatSession, SessionPage, SessionSearchResult } from "../types";
import {
  cleanupDispatcherSession,
  withDispatcherSessionRunning,
  withDispatcherSessionsRunning,
} from "./dispatcherSessionStore";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import { ChatCategorySection } from "./ChatCategorySection";
import { ChatNewCategoryDialog } from "./ChatNewCategoryDialog";
import s from "../styles";

const EXPANDED_STORAGE_KEY = "nezha.chat.expandedCategories";
const DEFAULT_CHAT_CATEGORY = "tech";
const CHAT_PAGE_SIZE = 30;

function sortSessionsByUpdatedAt(sessions: ChatSession[]) {
  return [...sessions].sort((left, right) => {
    const leftTime = Date.parse(left.updatedAt);
    const rightTime = Date.parse(right.updatedAt);
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });
}

function loadExpanded(): Set<string> {
  try {
    const raw = localStorage.getItem(EXPANDED_STORAGE_KEY);
    return raw ? new Set(JSON.parse(raw) as string[]) : new Set<string>();
  } catch {
    return new Set();
  }
}

function saveExpanded(set: Set<string>) {
  try {
    localStorage.setItem(EXPANDED_STORAGE_KEY, JSON.stringify([...set]));
  } catch {
    // ignore
  }
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
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [categories, setCategories] = useState<ChatCategory[]>([]);
  const [expandedRaw, setExpandedRaw] = useState<Set<string>>(() => loadExpanded());
  const [hasMore, setHasMore] = useState(false);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [expandedInitialized, setExpandedInitialized] = useState(false);
  const [showNewCategoryDialog, setShowNewCategoryDialog] = useState(false);
  const [editingCategory, setEditingCategory] = useState<ChatCategory | null>(null);
  const [dragOverCategoryId, setDragOverCategoryId] = useState<string | null>(null);
  const draggedSessionIdRef = useRef<string | null>(null);
  const creatingSessionRef = useRef(false);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;

  const loadInitial = useCallback(async () => {
    const [page, cats] = await Promise.all([
      invoke<SessionPage<ChatSession>>("chat_list_sessions", {
        category: null,
        cursor: null,
        pageSize: CHAT_PAGE_SIZE,
      }),
      invoke<ChatCategory[]>("chat_list_categories"),
    ]);
    const items = withDispatcherSessionsRunning(page.items);
    setSessions(items);
    setHasMore(page.hasMore);
    setNextCursor(page.nextCursor ?? null);
    setCategories(cats);
    return items;
  }, []);

  const loadMore = useCallback(async () => {
    if (!hasMore || !nextCursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await invoke<SessionPage<ChatSession>>("chat_list_sessions", {
        category: null,
        cursor: nextCursor,
        pageSize: CHAT_PAGE_SIZE,
      });
      setSessions((prev) => {
        const existing = new Set(prev.map((s) => s.id));
        const newItems = withDispatcherSessionsRunning(page.items.filter((s) => !existing.has(s.id)));
        return sortSessionsByUpdatedAt([...prev, ...newItems]);
      });
      setHasMore(page.hasMore);
      setNextCursor(page.nextCursor ?? null);
    } finally {
      setLoadingMore(false);
    }
  }, [hasMore, nextCursor, loadingMore]);

  useEffect(() => {
    const handleNew = async () => {
      try {
        const session = withDispatcherSessionRunning(await invoke<ChatSession>("chat_create_session", {
          title: "新聊天",
          category: DEFAULT_CHAT_CATEGORY,
        }));
        setSessions((prev) =>
          prev.some((s) => s.id === session.id) ? prev : sortSessionsByUpdatedAt([session, ...prev]),
        );
        onActiveSessionChange(session.id);
      } catch (err) {
        console.error("创建聊天失败:", err);
      }
    };

    let cancelled = false;

    loadInitial().then((loaded) => {
      if (cancelled) return;
      const current = activeSessionIdRef.current;
      if (!current && loaded.length === 0) {
        handleNew();
      } else if (loaded.length > 0 && (!current || !loaded.some((s) => s.id === current))) {
        onActiveSessionChange(loaded[0].id);
      }
      setExpandedInitialized(true);
    }).catch((err) => console.error("加载聊天失败:", err));

    return () => { cancelled = true; };
  }, [onActiveSessionChange, loadInitial]);

  useEffect(() => {
    if (!sentinelRef.current) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasMore && !loadingMore) {
          loadMore();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(sentinelRef.current);
    return () => observer.disconnect();
  }, [hasMore, loadingMore, loadMore]);

  useEffect(() => {
    const unlisten = listen<ChatSession>("dispatcher-session-updated", (event) => {
      const u = event.payload;
      if (!u.category && u.id) return;
      const updated = withDispatcherSessionRunning(u);
      setSessions((prev) => {
        const exists = prev.some((s) => s.id === updated.id);
        const next = exists
          ? prev.map((s) => (s.id === updated.id ? updated : s))
          : [updated, ...prev];
        return sortSessionsByUpdatedAt(next);
      });
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<ChatCategory>("chat-category-updated", (event) => {
      const updated = event.payload;
      setCategories((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!expandedInitialized || activeSessionId == null || sessions.length === 0) return;
    const active = sessions.find((s) => s.id === activeSessionId);
    if (!active) return;
    const catId = active.category || "__uncategorized__";
    if (!expandedRaw.has(catId)) {
      const next = new Set(expandedRaw);
      next.add(catId);
      setExpandedRaw(next);
      saveExpanded(next);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expandedInitialized, activeSessionId]);

  const sessionIds = useMemo(() => sessions.map((s) => s.id), [sessions]);
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

  const filtered = useMemo(() => {
    if (!query.trim()) return sessions;
    const q = query.toLowerCase();
    return sessions.filter((s) => s.title.toLowerCase().includes(q));
  }, [sessions, query]);

  const sessionsByCategory = useMemo(() => {
    const map = new Map<string, ChatSession[]>();
    for (const s of filtered) {
      const key = s.category || "__uncategorized__";
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(s);
    }
    return map;
  }, [filtered]);

  const handleCreateSession = async (categoryId?: string) => {
    if (creatingSessionRef.current) return;
    creatingSessionRef.current = true;
    try {
      const session = withDispatcherSessionRunning(await invoke<ChatSession>("chat_create_session", {
        title: "新聊天",
        category: categoryId || DEFAULT_CHAT_CATEGORY,
      }));
      setSessions((prev) =>
        prev.some((s) => s.id === session.id)
          ? prev
          : sortSessionsByUpdatedAt([session, ...prev]),
      );
      onActiveSessionChange(session.id);
      if (categoryId) {
        const next = new Set(expandedRaw);
        next.add(categoryId);
        setExpandedRaw(next);
        saveExpanded(next);
      }
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
    try {
      await invoke("chat_delete_session", { sessionId });
      cleanupDispatcherSession(sessionId);
      const remaining = sessions.filter((s) => s.id !== sessionId);
      setSessions(remaining);
      if (activeSessionId === sessionId) {
        onActiveSessionChange(remaining[0]?.id ?? null);
      }
      if (remaining.length === 0) {
        await handleCreateSession();
      }
    } catch (err) {
      console.error("删除聊天失败:", err);
    }
  };

  const toggleExpanded = (catId: string) => {
    const next = new Set(expandedRaw);
    if (next.has(catId)) {
      next.delete(catId);
    } else {
      next.add(catId);
    }
    setExpandedRaw(next);
    saveExpanded(next);
  };

  const handleCategoryDragStart = (sessionId: string, e: React.DragEvent) => {
    draggedSessionIdRef.current = sessionId;
    e.dataTransfer.setData("text/plain", sessionId);
    e.dataTransfer.effectAllowed = "move";
  };

  const handleCategoryDragOver = (_e: React.DragEvent) => {
    // Allow drop
  };

  const handleCategoryDrop = async (categoryId: string, e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const sessionId = e.dataTransfer.getData("text/plain") || draggedSessionIdRef.current;
    setDragOverCategoryId(null);
    draggedSessionIdRef.current = null;
    if (!sessionId) return;
    try {
      await invoke("chat_set_session_category_v6", {
        sessionId,
        categoryId: categoryId === "__uncategorized__" ? "" : categoryId,
      });
      setSessions((prev) =>
        prev.map((s) =>
          s.id === sessionId
            ? { ...s, category: categoryId === "__uncategorized__" ? "" : categoryId }
            : s,
        ),
      );
      const next = new Set(expandedRaw);
      next.add(categoryId);
      setExpandedRaw(next);
      saveExpanded(next);
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
        setCategories((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
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
      setCategories((prev) => prev.filter((c) => c.id !== cat.id));
      setSessions((prev) =>
        prev.map((s) => (s.category === cat.id ? { ...s, category: "" } : s)),
      );
    } catch (err) {
      console.error("删除分类失败:", err);
    }
  };

  const sortedCategories = useMemo(
    () => [...categories].sort((a, b) => a.sortOrder - b.sortOrder),
    [categories],
  );

  const uncategorizedSessions = sessionsByCategory.get("__uncategorized__") ?? [];
  const showUncategorized = uncategorizedSessions.length > 0;

  return (
    <div style={s.chatSessionPanel}>
      <div style={s.panelHeader}>
        <div style={s.chatPanelIcon}>
          <MessageCircle size={15} />
        </div>
        <span style={s.panelProjectName}>聊天</span>
      </div>

      <div style={s.panelSearchWrap}>
        <Search size={13} strokeWidth={2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
        <input
          style={s.panelSearchInput}
          placeholder="搜索聊天..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      <div style={s.taskActionsRow}>
        <button style={s.chatNewSessionBtn} onClick={() => handleCreateSession()}>
          <Plus size={14} strokeWidth={2.5} />
          新建聊天
        </button>
        {showBrowserButton && (
          <button
            style={s.chatNewSessionBtn}
            onClick={onToggleBrowser}
            title="CloakBrowser"
          >
            <MonitorDot size={14} strokeWidth={2.5} />
            浏览器
          </button>
        )}
      </div>

      <div style={s.taskDivider} />

      <div style={s.taskListScroll as React.CSSProperties}>
        {searchResults !== null ? (
          searchResults.length === 0 ? (
            <div style={s.taskListEmpty}>没有找到匹配的聊天</div>
          ) : (
            searchResults.map((r) => {
              return (
                <button
                  key={r.sessionId}
                  type="button"
                  style={{
                    ...s.sessionCard,
                    ...(r.sessionId === activeSessionId ? { background: "var(--bg-selected)" } : {}),
                    textAlign: "left" as const,
                    cursor: "pointer",
                    width: "calc(100% - 16px)",
                    border: "none",
                  }}
                  onClick={() => onActiveSessionChange(r.sessionId)}
                >
                  <div style={s.sessionCardBody as React.CSSProperties}>
                    <div style={s.sessionCardTitle}>{r.sessionTitle}</div>
                    {r.matchedKeywords.length > 0 && (
                      <div style={s.matchedKeywordsRow}>
                        {r.matchedKeywords.slice(0, 4).map((kw) => (
                          <span key={kw} style={s.matchedKeywordTag}>
                            {kw}
                          </span>
                        ))}
                        {r.matchedKeywords.length > 4 && (
                          <span style={{ ...s.matchedKeywordTag, opacity: 0.6 }}>
                            +{r.matchedKeywords.length - 4}
                          </span>
                        )}
                      </div>
                    )}
                  </div>
                </button>
              );
            })
          )
        ) : (
          <>
            {sortedCategories.map((cat) => {
              const catId = cat.id;
              const catSessions = sessionsByCategory.get(catId) ?? [];
              return (
                <ChatCategorySection
                  key={catId}
                  category={cat}
                  sessions={catSessions}
                  activeSessionId={activeSessionId}
                  runningSessionIds={runningSessionIds}
                  isExpanded={expandedRaw.has(catId)}
                  onToggle={() => toggleExpanded(catId)}
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
              );
            })}

            {showUncategorized && (
              <ChatCategorySection
                category={null}
                sessions={uncategorizedSessions}
                activeSessionId={activeSessionId}
                runningSessionIds={runningSessionIds}
                isExpanded={expandedRaw.has("__uncategorized__")}
                onToggle={() => toggleExpanded("__uncategorized__")}
                onSessionClick={onActiveSessionChange}
                onSessionDelete={handleDeleteSession}
                onSessionDragStart={handleCategoryDragStart}
                onNewInCategory={() => {}}
                onRenameCategory={() => {}}
                onDeleteCategory={() => {}}
                onDragOver={(e) => {
                  e.preventDefault();
                  setDragOverCategoryId("__uncategorized__");
                }}
                onDrop={(e) => handleCategoryDrop("__uncategorized__", e)}
                dragOverId={dragOverCategoryId}
              />
            )}

            {hasMore && (
              <div ref={sentinelRef} style={{ height: 1, width: "100%" }} />
            )}

            {filtered.length === 0 && (
              <div style={s.taskListEmpty}>没有找到聊天</div>
            )}
          </>
        )}
      </div>

      <div style={s.categoryFooterRow as React.CSSProperties}>
        <button
          style={s.categoryNewBtn}
          onClick={() => setShowNewCategoryDialog(true)}
        >
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
