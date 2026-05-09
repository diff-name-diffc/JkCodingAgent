import { RefreshCw, RotateCcw, Search, XCircle } from "lucide-react";
import type { KnowledgeIngestJob, KnowledgePageSummary, KnowledgeSearchResult } from "../../types";
import s from "../../styles";

export function PageList({
  groupedPages,
  selectedPage,
  onSelect,
}: {
  groupedPages: Array<[string, KnowledgePageSummary[]]>;
  selectedPage: KnowledgePageSummary | null;
  onSelect: (page: KnowledgePageSummary) => void;
}) {
  return (
    <aside style={s.knowledgePageList}>
      {groupedPages.length === 0 ? (
        <div style={{ padding: 12, color: "var(--text-muted)", fontSize: 12 }}>暂无页面，先导入源文件。</div>
      ) : (
        groupedPages.map(([type, items]) => (
          <section key={type} style={{ marginBottom: 12 }}>
            <div style={{ padding: "4px 6px", fontSize: 11, fontWeight: 800, color: "var(--text-hint)", textTransform: "uppercase" }}>
              {type} · {items.length}
            </div>
            {items.map((page) => {
              const active = selectedPage?.relativePath === page.relativePath;
              return (
                <button
                  key={page.relativePath}
                  style={{
                    ...s.knowledgePageItem,
                    background: active ? "var(--bg-selected)" : "transparent",
                    borderColor: active ? "var(--border-medium)" : "transparent",
                  }}
                  onClick={() => onSelect(page)}
                >
                  <div style={{ fontSize: 12.5, fontWeight: 700, color: "var(--text-primary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {page.title}
                  </div>
                  <div style={{ marginTop: 4, fontSize: 10.5, color: "var(--text-muted)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {page.relativePath}
                  </div>
                </button>
              );
            })}
          </section>
        ))
      )}
    </aside>
  );
}

export function SearchPanel({
  query,
  setQuery,
  results,
  busy,
  onSearch,
  onOpen,
}: {
  query: string;
  setQuery: (value: string) => void;
  results: KnowledgeSearchResult[];
  busy: boolean;
  onSearch: () => void;
  onOpen: (result: KnowledgeSearchResult) => void;
}) {
  return (
    <div style={s.knowledgeForm}>
      <div style={{ display: "flex", gap: 10 }}>
        <input
          style={{ ...s.knowledgeInput, flex: 1 }}
          value={query}
          placeholder="搜索知识库"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void onSearch();
          }}
        />
        <button style={s.knowledgeSmallBtn} onClick={onSearch} disabled={busy}>
          <Search size={14} />
          搜索
        </button>
      </div>
      <div style={{ display: "grid", gap: 10 }}>
        {results.map((result) => (
          <button key={`${result.collectionId}-${result.relativePath}`} style={{ ...s.knowledgeCard, textAlign: "left", cursor: "pointer" }} onClick={() => onOpen(result)}>
            <div style={{ fontSize: 13.5, fontWeight: 750, color: "var(--text-primary)" }}>{result.title}</div>
            <div style={{ marginTop: 4, fontSize: 11, color: "var(--text-muted)" }}>
              {result.pageType} · score {result.score.toFixed(3)} · {result.relativePath}
            </div>
            <div style={{ marginTop: 10, fontSize: 12.5, color: "var(--text-secondary)", lineHeight: 1.55 }}>{result.snippet}</div>
          </button>
        ))}
      </div>
    </div>
  );
}

export function JobsPanel({
  jobs,
  busy,
  onRefresh,
  onCancel,
  onRetry,
}: {
  jobs: KnowledgeIngestJob[];
  busy: boolean;
  onRefresh: () => void;
  onCancel: (jobId: string) => void;
  onRetry: (jobId: string) => void;
}) {
  return (
    <div style={s.knowledgeForm}>
      <button style={s.knowledgeSmallBtn} onClick={onRefresh} disabled={busy}>
        <RefreshCw size={14} />
        刷新任务
      </button>
      <div style={{ display: "grid", gap: 10 }}>
        {jobs.length === 0 ? (
          <div style={{ color: "var(--text-muted)", fontSize: 13 }}>暂无导入任务。</div>
        ) : (
          jobs.map((job) => {
            const canCancel = job.status === "pending" || job.status === "running";
            const canRetry = job.status === "failed" || job.status === "cancelled";
            return (
              <div key={job.id} style={s.knowledgeCard}>
                <div style={{ display: "flex", justifyContent: "space-between", gap: 10 }}>
                  <div style={{ fontSize: 13, fontWeight: 750, color: "var(--text-primary)" }}>{job.sourceName}</div>
                  <div style={{ fontSize: 11, fontWeight: 800, color: "var(--text-muted)" }}>{job.status}</div>
                </div>
                <div style={{ marginTop: 6, fontSize: 12, color: "var(--text-muted)" }}>{job.message}</div>
                <div style={{ marginTop: 10, display: "flex", justifyContent: "space-between", gap: 8 }}>
                  <div style={{ fontSize: 11, color: "var(--text-hint)" }}>
                    {job.pagesWritten.length > 0 ? `${job.pagesWritten.length} pages` : job.sourcePath}
                  </div>
                  <div style={{ display: "flex", gap: 6 }}>
                    {canCancel && (
                      <button style={s.knowledgeSmallBtn} onClick={() => onCancel(job.id)} disabled={busy}>
                        <XCircle size={13} />
                        取消
                      </button>
                    )}
                    {canRetry && (
                      <button style={s.knowledgeSmallBtn} onClick={() => onRetry(job.id)} disabled={busy}>
                        <RotateCcw size={13} />
                        重试
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
