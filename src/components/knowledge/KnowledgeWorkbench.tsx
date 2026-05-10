import { useEffect, useMemo, useRef, useState } from "react";
import type React from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import Editor from "@monaco-editor/react";
import {
  BookOpen,
  GitGraph,
  RefreshCw,
  Save,
  Search,
  Settings,
  Upload,
  ListChecks,
} from "lucide-react";
import type {
  KnowledgeCollection,
  KnowledgeGraph,
  KnowledgeIngestJob,
  KnowledgePageContent,
  KnowledgePageSummary,
  KnowledgeSearchResult,
  KnowledgeSettings,
  KnowledgeVectorStats,
} from "../../types";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { KnowledgeGraphView } from "./KnowledgeGraphView";
import { JobsPanel, PageList, SearchPanel } from "./KnowledgePanels";
import { KnowledgeSettingsPanel } from "./KnowledgeSettingsPanel";
import { resolveKnowledgeImageUrls } from "./knowledgeImageUrls";
import s from "../../styles";

type KnowledgeTab = "pages" | "search" | "graph" | "settings" | "jobs";

export function KnowledgeWorkbench({
  collection,
  settings,
  onSettingsSaved,
  onImportSources,
  importRefreshToken,
}: {
  collection: KnowledgeCollection | null;
  settings: KnowledgeSettings;
  onSettingsSaved: (settings: KnowledgeSettings) => void;
  onImportSources: () => void;
  importRefreshToken: number;
}) {
  const [tab, setTab] = useState<KnowledgeTab>("pages");
  const [pages, setPages] = useState<KnowledgePageSummary[]>([]);
  const [selectedPage, setSelectedPage] = useState<KnowledgePageSummary | null>(null);
  const [pageContent, setPageContent] = useState<KnowledgePageContent | null>(null);
  const [draft, setDraft] = useState("");
  const [jobs, setJobs] = useState<KnowledgeIngestJob[]>([]);
  const [stats, setStats] = useState<KnowledgeVectorStats | null>(null);
  const [graph, setGraph] = useState<KnowledgeGraph | null>(null);
  const [graphSelectedPath, setGraphSelectedPath] = useState<string | null>(null);
  const [graphPageContent, setGraphPageContent] = useState<KnowledgePageContent | null>(null);
  const [graphPageLoading, setGraphPageLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<KnowledgeSearchResult[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const selectedRelativePath = selectedPage?.relativePath ?? null;

  const prevImportTokenRef = useRef(importRefreshToken);

  useEffect(() => {
    const collectionId = collection?.id;
    const isImportRefresh = importRefreshToken !== prevImportTokenRef.current;
    prevImportTokenRef.current = importRefreshToken;

    if (!collectionId) return;

    if (!isImportRefresh) {
      setSelectedPage(null);
      setPageContent(null);
      setDraft("");
      setGraph(null);
      setGraphSelectedPath(null);
      setGraphPageContent(null);
      setSearchResults([]);
    }
    void loadCollectionData(collectionId);
  }, [collection?.id, importRefreshToken]);

  useEffect(() => {
    const collectionId = collection?.id;
    if (!collectionId || !selectedRelativePath) return;
    invoke<KnowledgePageContent>("knowledge_read_page", {
      collectionId,
      relativePath: selectedRelativePath,
    })
      .then((page) => {
        setPageContent(page);
        setDraft(page.content);
      })
      .catch((error) => setMessage(String(error)));
  }, [collection?.id, selectedRelativePath]);

  useEffect(() => {
    const collectionId = collection?.id;
    if (!collectionId || tab !== "graph" || !graphSelectedPath) return;

    let cancelled = false;
    setGraphPageLoading(true);
    setGraphPageContent(null);
    invoke<KnowledgePageContent>("knowledge_read_page", {
      collectionId,
      relativePath: graphSelectedPath,
    })
      .then((page) => {
        if (!cancelled) setGraphPageContent(page);
      })
      .catch((error) => {
        if (!cancelled) setMessage(String(error));
      })
      .finally(() => {
        if (!cancelled) setGraphPageLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [collection?.id, graphSelectedPath, tab]);

  async function loadPages() {
    if (!collection) return;
    const nextPages = await invoke<KnowledgePageSummary[]>("knowledge_list_pages", {
      collectionId: collection.id,
    });
    setPages(nextPages);
    if (selectedPage) {
      setSelectedPage(
        nextPages.find((page) => page.relativePath === selectedPage.relativePath) ?? null,
      );
    }
  }

  async function loadCollectionData(collectionId: string) {
    const results = await Promise.allSettled([
      invoke<KnowledgePageSummary[]>("knowledge_list_pages", { collectionId }),
      invoke<KnowledgeIngestJob[]>("knowledge_get_ingest_jobs", { collectionId }),
      invoke<KnowledgeVectorStats>("knowledge_vector_stats", { collectionId }),
    ]);
    if (results[0].status === "fulfilled") setPages(results[0].value);
    if (results[1].status === "fulfilled") setJobs(results[1].value);
    if (results[2].status === "fulfilled") setStats(results[2].value);
    const errors = results
      .filter((r): r is PromiseRejectedResult => r.status === "rejected")
      .map((r) => String(r.reason));
    if (errors.length > 0) setMessage(errors.join("; "));
  }

  async function loadJobs() {
    if (!collection) return;
    const nextJobs = await invoke<KnowledgeIngestJob[]>("knowledge_get_ingest_jobs", {
      collectionId: collection.id,
    });
    setJobs(nextJobs);
  }

  async function savePage() {
    if (!collection || !pageContent) return;
    setBusy(true);
    setMessage(null);
    try {
      const saved = await invoke<KnowledgePageContent>("knowledge_write_page", {
        collectionId: collection.id,
        relativePath: pageContent.relativePath,
        content: draft,
      });
      setPageContent(saved);
      await loadPages();
      setMessage("页面已保存");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function runSearch() {
    if (!collection || !query.trim()) return;
    setBusy(true);
    setMessage(null);
    try {
      const results = await invoke<KnowledgeSearchResult[]>("knowledge_search", {
        query,
        collectionIds: [collection.id],
        limit: 20,
      });
      setSearchResults(results);
    } catch (error) {
      setSearchResults([]);
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function reindex() {
    if (!collection) return;
    setBusy(true);
    setMessage("正在重建索引...");
    try {
      const nextStats = await invoke<KnowledgeVectorStats>("knowledge_reindex_collection", {
        collectionId: collection.id,
      });
      setStats(nextStats);
      setMessage(`索引完成：${nextStats.pageCount} 页，${nextStats.chunkCount} chunks`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function buildGraph() {
    if (!collection) return;
    setBusy(true);
    setMessage(null);
    try {
      const nextGraph = await invoke<KnowledgeGraph>("knowledge_build_graph", {
        collectionId: collection.id,
      });
      setGraph(nextGraph);
      if (graphSelectedPath && !nextGraph.nodes.some((node) => node.path === graphSelectedPath)) {
        setGraphSelectedPath(null);
        setGraphPageContent(null);
      }
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function cancelJob(jobId: string) {
    setBusy(true);
    setMessage(null);
    try {
      await invoke<KnowledgeIngestJob>("knowledge_cancel_ingest", { jobId });
      await loadJobs();
      setMessage("任务已取消");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function retryJob(jobId: string) {
    if (!collection) return;
    setBusy(true);
    setMessage("正在重试导入...");
    try {
      await invoke<KnowledgeIngestJob>("knowledge_retry_ingest", { jobId });
      await loadCollectionData(collection.id);
      setMessage("重试完成");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  const groupedPages = useMemo(() => {
    const grouped = new Map<string, KnowledgePageSummary[]>();
    for (const page of pages) {
      const list = grouped.get(page.pageType) ?? [];
      list.push(page);
      grouped.set(page.pageType, list);
    }
    return [...grouped.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [pages]);

  if (!collection) {
    return (
      <main style={s.knowledgeMain}>
        <div style={{ ...s.emptyState, background: "var(--bg-panel)" }}>
          <BookOpen size={42} color="var(--text-hint)" />
          <div
            style={{ marginTop: 14, fontSize: 14, fontWeight: 700, color: "var(--text-secondary)" }}
          >
            选择或创建一个知识库集合
          </div>
        </div>
      </main>
    );
  }

  return (
    <main style={s.knowledgeMain}>
      <div style={s.knowledgeTopbar}>
        <div style={{ minWidth: 0 }}>
          <div style={s.knowledgeTitle}>{collection.name}</div>
          <div style={s.knowledgeSubtitle}>
            {stats
              ? `${stats.pageCount} 页 · ${stats.chunkCount} chunks · ${stats.dimension || 0} 维`
              : "知识库集合"}
          </div>
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <button
            style={s.knowledgeSmallBtn}
            onClick={onImportSources}
            disabled={busy}
            aria-label="导入源文件"
          >
            <Upload size={14} />
            导入
          </button>
          <button style={s.knowledgeSmallBtn} onClick={reindex} disabled={busy}>
            <RefreshCw size={14} />
            重建索引
          </button>
        </div>
      </div>

      <div style={{ ...s.knowledgeTopbar, height: 48, justifyContent: "space-between" }}>
        <div style={s.knowledgeTabs}>
          <TabButton
            icon={<BookOpen size={14} />}
            active={tab === "pages"}
            label="页面"
            onClick={() => setTab("pages")}
          />
          <TabButton
            icon={<Search size={14} />}
            active={tab === "search"}
            label="搜索"
            onClick={() => setTab("search")}
          />
          <TabButton
            icon={<GitGraph size={14} />}
            active={tab === "graph"}
            label="图谱"
            onClick={() => {
              setTab("graph");
              void buildGraph();
            }}
          />
          <TabButton
            icon={<Settings size={14} />}
            active={tab === "settings"}
            label="设置"
            onClick={() => setTab("settings")}
          />
          <TabButton
            icon={<ListChecks size={14} />}
            active={tab === "jobs"}
            label="任务"
            onClick={() => {
              setTab("jobs");
              void loadJobs();
            }}
          />
        </div>
        {message && <span style={{ fontSize: 12, color: "var(--text-muted)" }}>{message}</span>}
      </div>

      {tab === "settings" ? (
        <KnowledgeSettingsPanel settings={settings} onSettingsSaved={onSettingsSaved} />
      ) : tab === "search" ? (
        <SearchPanel
          query={query}
          setQuery={setQuery}
          results={searchResults}
          busy={busy}
          onSearch={runSearch}
          onOpen={(result) => {
            setTab("pages");
            const page =
              result.collectionId === collection.id
                ? pages.find((item) => item.relativePath === result.relativePath)
                : null;
            setSelectedPage(
              page ?? {
                collectionId: result.collectionId,
                path: result.path,
                relativePath: result.relativePath,
                title: result.title,
                pageType: result.pageType,
                tags: [],
              },
            );
          }}
        />
      ) : tab === "graph" ? (
        <KnowledgeGraphView
          graph={graph}
          selectedPath={graphSelectedPath}
          selectedPage={graphPageContent}
          pageLoading={graphPageLoading}
          onSelectPage={setGraphSelectedPath}
          onClosePage={() => {
            setGraphSelectedPath(null);
            setGraphPageContent(null);
          }}
          renderPageContent={(content) => (
            <MarkdownRenderer
              content={resolveKnowledgeImageUrls(content, collection.rootPath, convertFileSrc)}
              variant="document"
            />
          )}
        />
      ) : tab === "jobs" ? (
        <JobsPanel
          jobs={jobs}
          busy={busy}
          onRefresh={loadJobs}
          onCancel={cancelJob}
          onRetry={retryJob}
        />
      ) : (
        <div style={s.knowledgeContent}>
          <PageList
            groupedPages={groupedPages}
            selectedPage={selectedPage}
            onSelect={setSelectedPage}
          />
          <div style={s.knowledgeEditorPane}>
            <div style={s.knowledgeEditorColumn}>
              <div style={{ ...s.knowledgeTopbar, height: 44 }}>
                <span style={{ fontSize: 12, fontWeight: 700, color: "var(--text-secondary)" }}>
                  {pageContent?.relativePath ?? "未选择页面"}
                </span>
                <button
                  style={s.knowledgeSmallBtn}
                  onClick={savePage}
                  disabled={!pageContent || busy}
                >
                  <Save size={14} />
                  保存
                </button>
              </div>
              <div style={{ flex: 1, minHeight: 0 }}>
                <Editor
                  key={pageContent?.relativePath ?? "__empty__"}
                  language="markdown"
                  theme="vs-dark"
                  value={draft}
                  onChange={(value) => setDraft(value ?? "")}
                  options={{ minimap: { enabled: false }, wordWrap: "on", fontSize: 13 }}
                />
              </div>
            </div>
            <div style={s.knowledgePreview}>
              {draft ? (
                <MarkdownRenderer
                  content={resolveKnowledgeImageUrls(draft, collection.rootPath, convertFileSrc)}
                  variant="document"
                />
              ) : (
                <div style={{ color: "var(--text-muted)", fontSize: 13 }}>选择页面后查看预览。</div>
              )}
            </div>
          </div>
        </div>
      )}
    </main>
  );
}

function TabButton({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      style={{
        ...s.knowledgeTab,
        background: active ? "var(--bg-selected)" : "transparent",
        color: active ? "var(--text-primary)" : "var(--text-muted)",
        borderColor: active ? "var(--border-medium)" : "transparent",
      }}
      onClick={onClick}
    >
      {icon}
      {label}
    </button>
  );
}
