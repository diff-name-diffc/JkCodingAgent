import { useState, useEffect, useCallback } from "react";
import * as Select from "@radix-ui/react-select";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { RotateCw, Check, RefreshCw, FileText, Upload, X, ChevronDown } from "lucide-react";
import s from "../../../styles";
import { useToast } from "../../Toast";
import type {
  RagIngestJobStartResult,
  RagIngestJobStatus,
  RagKbConfig,
  RagKbSaveResult,
  RagRuntimeStatus,
} from "../../../types";
import { PasswordInput } from "./PasswordInput";
import { RagSidecarLogPanel } from "./RagSidecarLogPanel";

interface RagKbConfigPanelProps {
  /** 与 AhaAgentPanel 对齐，预留按项目隔离扩展（当前 RAG 配置为全局）。 */
  projectId?: string;
  projectPath?: string;
}

/** 脏检查：逐字段比较两份配置是否一致。 */
function isConfigDirty(a: RagKbConfig, b: RagKbConfig): boolean {
  return JSON.stringify(a) !== JSON.stringify(b);
}

/** 区块内联测试反馈。 */
interface TestFeedback {
  status: "success" | "error";
  message: string;
}

interface RagVectorTestResult {
  ok: boolean;
  message?: string;
  status?: number;
  dimension?: number;
}

const LOG_LEVELS = [
  { value: "DEBUG", label: "Debug" },
  { value: "INFO", label: "Info" },
  { value: "WARNING", label: "Warning" },
  { value: "ERROR", label: "Error" },
];

const RAG_FILE_EXTENSIONS = [
  "pdf",
  "docx",
  "pptx",
  "md",
  "markdown",
  "txt",
  "html",
  "htm",
  "csv",
  "xlsx",
  "png",
  "jpg",
  "jpeg",
  "webp",
  "bmp",
];

const SPARSE_PROVIDER_OPTIONS = [
  { value: "fastembed", label: "FastEmbed" },
];

const SPARSE_MODEL_OPTIONS_BY_PROVIDER: Record<string, Array<{ value: string; label: string }>> = {
  fastembed: [
    { value: "Qdrant/bm25", label: "Qdrant/bm25" },
    { value: "Qdrant/minicoil-v1", label: "Qdrant/minicoil-v1" },
    {
      value: "Qdrant/bm42-all-minilm-l6-v2-attentions",
      label: "Qdrant/bm42-all-minilm-l6-v2-attentions",
    },
    { value: "prithivida/Splade_PP_en_v1", label: "prithivida/Splade_PP_en_v1" },
    { value: "prithvida/Splade_PP_en_v1", label: "prithvida/Splade_PP_en_v1" },
  ],
};

function normalizeLogLevel(value: string): string {
  const normalized = value.trim().toUpperCase();
  return LOG_LEVELS.some((item) => item.value === normalized) ? normalized : "INFO";
}

function normalizeSparseConfig(config: RagKbConfig): RagKbConfig {
  const provider = SPARSE_PROVIDER_OPTIONS.some((option) => option.value === config.sparseEmbedding.provider)
    ? config.sparseEmbedding.provider
    : SPARSE_PROVIDER_OPTIONS[0].value;
  const modelOptions = SPARSE_MODEL_OPTIONS_BY_PROVIDER[provider] ?? [];
  const model =
    config.sparseEmbedding.model === "Qdrant/BM25"
      ? "Qdrant/bm25"
      : config.sparseEmbedding.model;
  return {
    ...config,
    sparseEmbedding: {
      provider,
      model: modelOptions.some((option) => option.value === model)
        ? model
        : modelOptions[0]?.value ?? model,
    },
  };
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function EnumSelect({
  value,
  options,
  onValueChange,
  placeholder,
}: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onValueChange: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <Select.Root value={value} onValueChange={onValueChange}>
      <Select.Trigger
        style={{
          ...s.ahaInput,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          textAlign: "left",
          cursor: "pointer",
        }}
      >
        <Select.Value placeholder={placeholder} />
        <Select.Icon asChild>
          <ChevronDown size={14} color="var(--text-hint)" />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Content
          position="popper"
          sideOffset={4}
          style={{
            zIndex: 10000,
            minWidth: "var(--radix-select-trigger-width)",
            maxWidth: 420,
            background: "var(--bg-card)",
            border: "1px solid var(--border-medium)",
            borderRadius: 8,
            boxShadow: "var(--shadow-md)",
            padding: 4,
          }}
        >
          <Select.Viewport>
            {options.map((option) => (
              <Select.Item
                key={option.value}
                value={option.value}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  minHeight: 30,
                  padding: "0 28px 0 10px",
                  borderRadius: 6,
                  color: "var(--text-primary)",
                  fontSize: 12,
                  cursor: "pointer",
                  position: "relative",
                  outline: "none",
                }}
              >
                <Select.ItemText>{option.label}</Select.ItemText>
                <Select.ItemIndicator
                  style={{
                    position: "absolute",
                    right: 8,
                    display: "inline-flex",
                    alignItems: "center",
                  }}
                >
                  <Check size={12} />
                </Select.ItemIndicator>
              </Select.Item>
            ))}
          </Select.Viewport>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}

export function RagKbConfigPanel({ projectId, projectPath }: RagKbConfigPanelProps) {
  const { showToast } = useToast();

  const [config, setConfig] = useState<RagKbConfig | null>(null);
  const [original, setOriginal] = useState<RagKbConfig | null>(null);
  const [loading, setLoading] = useState(true);

  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [runtimeStatus, setRuntimeStatus] = useState<RagRuntimeStatus>({ running: false });
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);

  const [qdrantTest, setQdrantTest] = useState<TestFeedback | null>(null);
  const [embeddingTest, setEmbeddingTest] = useState<TestFeedback | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [ingesting, setIngesting] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);
  const [ingestJob, setIngestJob] = useState<RagIngestJobStatus | null>(null);

  // 加载配置 + 运行状态
  useEffect(() => {
    invoke<RagKbConfig>("rag_get_kb_config")
      .then((loaded) => {
        const normalized = normalizeSparseConfig(loaded);
        setConfig(normalized);
        setOriginal(normalized);
      })
      .catch((e) => {
        setSaveError(String(e));
        showToast(`加载 RAG 配置失败：${String(e)}`);
      })
      .finally(() => setLoading(false));
    // 首次查询运行状态；若未运行（可能正在随主应用自动启动），
    // 短间隔重试几次以反映"启动中 → 已运行"的状态变化
    const poll = (retries: number) => {
      invoke<RagRuntimeStatus>("rag_status")
        .then((status) => {
          setRuntimeStatus(status);
          if (!status.running && retries > 0) {
            window.setTimeout(() => poll(retries - 1), 1500);
          }
        })
        .catch(() => {});
    };
    poll(5);
  }, [showToast]);

  // 局部更新工具：深入更新 qdrant / embedding 子对象
  const patchQdrant = (patch: Partial<RagKbConfig["qdrant"]>) => {
    setConfig((prev) =>
      prev ? { ...prev, qdrant: { ...prev.qdrant, ...patch } } : prev,
    );
  };
  const patchEmbedding = (patch: Partial<RagKbConfig["embedding"]>) => {
    setConfig((prev) =>
      prev ? { ...prev, embedding: { ...prev.embedding, ...patch } } : prev,
    );
  };
  const patchSparseEmbedding = (patch: Partial<RagKbConfig["sparseEmbedding"]>) => {
    setConfig((prev) =>
      prev
        ? { ...prev, sparseEmbedding: { ...prev.sparseEmbedding, ...patch } }
        : prev,
    );
  };
  const patchChunking = (patch: Partial<RagKbConfig["chunking"]>) => {
    setConfig((prev) =>
      prev ? { ...prev, chunking: { ...prev.chunking, ...patch } } : prev,
    );
  };
  const patchOcr = (patch: Partial<RagKbConfig["ocr"]>) => {
    setConfig((prev) => (prev ? { ...prev, ocr: { ...prev.ocr, ...patch } } : prev));
  };

  const dirty = config && original ? isConfigDirty(config, original) : false;

  const persistConfig = useCallback(
    async (nextConfig: RagKbConfig) => {
      const result = await invoke<RagKbSaveResult>("rag_save_kb_config", {
        config: nextConfig,
      });
      setOriginal(nextConfig);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
      if (result.reloaded) {
        showToast("配置已保存并热更新到运行中的服务", "warning");
      }
      return result;
    },
    [showToast],
  );

  // 保存配置
  const handleSave = useCallback(async () => {
    if (!config) return;
    setSaving(true);
    setSaved(false);
    setSaveError(null);
    try {
      await persistConfig(config);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      setSaving(false);
    }
  }, [config, persistConfig]);

  // sidecar 重启（随主应用自动启停，面板不暴露启动/停止）
  const handleRestart = useCallback(async () => {
    setActionInProgress("restart");
    try {
      await invoke("rag_stop");
      await invoke("rag_start");
      // 重启后立即查状态很可能是"启动中"，轮询几次直到就绪
      const poll = (retries: number) => {
        invoke<RagRuntimeStatus>("rag_status")
          .then((status) => {
            setRuntimeStatus(status);
            if (!status.running && retries > 0) {
              window.setTimeout(() => poll(retries - 1), 1500);
            }
          })
          .catch(() => {});
      };
      poll(5);
    } catch (error) {
      showToast(`重启 RAG 服务失败：${String(error)}`);
    } finally {
      setActionInProgress(null);
    }
  }, [showToast]);

  // 测试连接：先保存桌面端配置，再由无状态 sidecar 使用本次配置执行测试。
  const runTest = useCallback(
    async (target: "qdrant" | "embedding") => {
      if (!config) return;
      const setter = target === "qdrant" ? setQdrantTest : setEmbeddingTest;
      const command = target === "qdrant" ? "rag_test_qdrant" : "rag_test_embedding";
      setter({ status: "success", message: "测试中..." });
      setActionInProgress(`test-${target}`);
      setSaveError(null);
      try {
        await persistConfig(config);
        const result = await invoke<RagVectorTestResult>(command, { config });
        setter({ status: "success", message: result.message ?? "连接正常" });
        window.setTimeout(() => setter(null), 3000);
        invoke<RagRuntimeStatus>("rag_status")
          .then(setRuntimeStatus)
          .catch(() => {});
      } catch (error) {
        setter({ status: "error", message: String(error) });
      } finally {
        setActionInProgress(null);
      }
    },
    [config, persistConfig],
  );

  const pickIngestFiles = useCallback(async () => {
    const selected = await openDialog({
      directory: false,
      multiple: true,
      defaultPath: projectPath,
      title: "选择要导入 RAG 知识库的文件",
      filters: [{ name: "RAG 文档", extensions: RAG_FILE_EXTENSIONS }],
    });
    const files = Array.isArray(selected) ? selected : typeof selected === "string" ? [selected] : [];
    if (files.length > 0) {
      setSelectedFiles((prev) => Array.from(new Set([...prev, ...files])));
      setIngestError(null);
    }
  }, [projectPath]);

  const removeSelectedFile = useCallback((file: string) => {
    setSelectedFiles((prev) => prev.filter((item) => item !== file));
  }, []);

  const startIngest = useCallback(async () => {
    if (!config || !projectId || !projectPath || selectedFiles.length === 0) return;
    setIngesting(true);
    setIngestError(null);
    setIngestJob(null);
    try {
      await persistConfig(config);
      const started = await invoke<RagIngestJobStartResult>("rag_ingest_files", {
        projectId,
        projectPath,
        files: selectedFiles,
      });
      let done = false;
      while (!done) {
        const status = await invoke<RagIngestJobStatus>("rag_ingest_job_status", {
          jobId: started.jobId,
        });
        setIngestJob(status);
        done = ["done", "partial", "failed"].includes(status.status);
        if (!done) {
          await new Promise((resolve) => window.setTimeout(resolve, 1200));
        }
      }
    } catch (error) {
      setIngestError(String(error));
    } finally {
      setIngesting(false);
    }
  }, [config, persistConfig, projectId, projectPath, selectedFiles]);

  if (loading || !config) {
    return (
      <div style={{ ...s.ahaPanel, alignItems: "center", justifyContent: "center" }}>
        <span style={{ color: "var(--text-muted)", fontSize: 13 }}>加载中...</span>
      </div>
    );
  }

  const running = runtimeStatus.running;
  const importReady = Boolean(projectId && projectPath);
  const sparseModelOptions =
    SPARSE_MODEL_OPTIONS_BY_PROVIDER[config.sparseEmbedding.provider] ??
    SPARSE_MODEL_OPTIONS_BY_PROVIDER.fastembed;

  return (
    <>
      <div style={s.ahaPanel}>
        <div style={s.ahaBody}>
          <div style={s.ahaContent}>
            {/* ── 运行状态区 ── */}
            <div style={s.ragRuntimeBar}>
              <div style={s.ragRuntimeInfo}>
                <span
                  style={{
                    ...s.ragStatusDot,
                    background: running ? "var(--success)" : "var(--text-hint)",
                    boxShadow: running
                      ? "0 0 6px var(--success)"
                      : "none",
                  }}
                />
                <span style={s.ragStatusText}>
                  {running
                    ? `已运行 · 端口 ${runtimeStatus.port ?? "-"}`
                    : "启动中…"}
                </span>
              </div>
              <div style={s.ahaActionRow}>
                <button
                  type="button"
                  style={s.ahaGhostButton}
                  onClick={handleRestart}
                  disabled={actionInProgress !== null}
                  title="重启 RAG 服务"
                >
                  <RotateCw size={13} />
                  {actionInProgress === "restart" ? "重启中..." : "重启"}
                </button>
              </div>
            </div>

            {/* ── 服务日志区 ── */}
            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>服务日志</div>
                  <div style={s.ahaSectionDescription}>
                    日志只保留内存中的最近 2000 行。等级保存后会热更新到运行中的 sidecar。
                  </div>
                </div>
              </div>
              <div style={s.ahaField}>
                <span style={s.ahaLabel}>日志等级</span>
                <div style={s.ahaActionRow}>
                  {LOG_LEVELS.map((level) => {
                    const active = normalizeLogLevel(config.logLevel) === level.value;
                    return (
                      <button
                        key={level.value}
                        type="button"
                        style={active ? s.ahaActiveBadge : s.ahaInactiveBadge}
                        onClick={() =>
                          setConfig((prev) =>
                            prev ? { ...prev, logLevel: level.value } : prev,
                          )
                        }
                      >
                        {level.label}
                      </button>
                    );
                  })}
                </div>
                <span style={s.ahaHint}>
                  Debug 适合排查启动和连接问题；日常建议保持 Info 或 Warning。
                </span>
              </div>
              <RagSidecarLogPanel />
            </div>

            {/* ── 文档导入区 ── */}
            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>导入文档</div>
                  <div style={s.ahaSectionDescription}>
                    支持 PDF、Office、Markdown、文本、表格、HTML 与图片；文件必须位于当前项目目录内。
                  </div>
                </div>
                <div style={s.ahaActionRow}>
                  <button
                    type="button"
                    style={s.ahaGhostButton}
                    onClick={pickIngestFiles}
                    disabled={!importReady || ingesting}
                    title="选择文件"
                  >
                    <FileText size={13} />
                    选择
                  </button>
                  <button
                    type="button"
                    style={{
                      ...s.ahaGhostButton,
                      opacity: selectedFiles.length === 0 || !importReady || ingesting ? 0.5 : 1,
                    }}
                    onClick={startIngest}
                    disabled={selectedFiles.length === 0 || !importReady || ingesting}
                    title="导入知识库"
                  >
                    <Upload size={13} />
                    {ingesting ? "导入中..." : "导入"}
                  </button>
                </div>
              </div>
              {!importReady && (
                <span style={s.ahaHint}>请从具体项目打开设置后再导入文档。</span>
              )}
              {selectedFiles.length > 0 && (
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {selectedFiles.map((file) => (
                    <div
                      key={file}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        minHeight: 28,
                        color: "var(--text-secondary)",
                        fontSize: 12,
                      }}
                    >
                      <FileText size={13} />
                      <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>
                        {fileName(file)}
                      </span>
                      <button
                        type="button"
                        style={s.ragLogIconButton}
                        onClick={() => removeSelectedFile(file)}
                        disabled={ingesting}
                        title="移除"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  ))}
                </div>
              )}
              {ingestError && (
                <span style={{ ...s.ahaFeedback, color: "var(--danger)" }}>{ingestError}</span>
              )}
              {ingestJob && (
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  <span style={s.ahaHint}>
                    状态：{ingestJob.status} · {ingestJob.completedFiles}/{ingestJob.totalFiles} 完成
                    {ingestJob.failedFiles > 0 ? ` · ${ingestJob.failedFiles} 失败` : ""}
                  </span>
                  {ingestJob.files.map((file) => (
                    <span
                      key={file.path}
                      style={{
                        color: file.status === "failed" ? "var(--danger)" : "var(--text-secondary)",
                        fontSize: 12,
                      }}
                    >
                      {fileName(file.path)} · {file.status}
                      {file.status === "done"
                        ? ` · ${file.parentChunks} parent / ${file.indexedPoints} vectors`
                        : ""}
                      {file.error ? ` · ${file.error}` : ""}
                    </span>
                  ))}
                </div>
              )}
            </div>

            {/* ── Qdrant 配置区 ── */}
            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>Qdrant 向量库</div>
                  <div style={s.ahaSectionDescription}>
                    外部独立部署的 Qdrant 实例连接信息。配置存于本地，启动时注入子进程。
                  </div>
                </div>
                <button
                  type="button"
                  style={s.ahaGhostButton}
                  onClick={() => runTest("qdrant")}
                  disabled={actionInProgress !== null || saving}
                  title="测试连接"
                >
                  <RefreshCw size={13} />
                  测试连接
                </button>
              </div>
              {qdrantTest && (
                <span
                  style={{
                    ...s.ahaFeedback,
                    color:
                      qdrantTest.status === "success"
                        ? "var(--success)"
                        : "var(--danger)",
                  }}
                >
                  {qdrantTest.status === "success" ? <Check size={12} /> : null}{" "}
                  {qdrantTest.message}
                </span>
              )}
              <div style={s.ahaGrid}>
                <label style={s.ahaField}>
                  <span style={s.ahaLabel}>HTTP 端点</span>
                  <input
                    style={s.ahaInput}
                    value={config.qdrant.url}
                    onChange={(e) => patchQdrant({ url: e.target.value })}
                    placeholder="http://127.0.0.1:6333"
                    spellCheck={false}
                  />
                  <span style={s.ahaHint}>Qdrant 的 HTTP REST 端点地址。</span>
                </label>
                <label style={s.ahaField}>
                  <span style={s.ahaLabel}>API Key</span>
                  <PasswordInput
                    value={config.qdrant.apiKey}
                    onChange={(v) => patchQdrant({ apiKey: v })}
                    placeholder="可选，留空则不鉴权"
                  />
                </label>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>命名前缀</span>
                    <input
                      style={s.ahaInput}
                      value={config.qdrant.collectionPrefix}
                      onChange={(e) =>
                        patchQdrant({ collectionPrefix: e.target.value })
                      }
                      placeholder="jk_"
                      spellCheck={false}
                    />
                  </label>
                  <label style={{ ...s.ahaField, width: 120, flexShrink: 0 }}>
                    <span style={s.ahaLabel}>超时（秒）</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={1}
                      step={1}
                      value={config.qdrant.timeout}
                      onChange={(e) =>
                        patchQdrant({ timeout: Number(e.target.value) })
                      }
                    />
                  </label>
                </div>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>稠密向量名</span>
                    <input
                      style={s.ahaInput}
                      value={config.qdrant.denseVectorName}
                      onChange={(e) =>
                        patchQdrant({ denseVectorName: e.target.value })
                      }
                      placeholder="dense"
                      spellCheck={false}
                    />
                  </label>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>稀疏向量名</span>
                    <input
                      style={s.ahaInput}
                      value={config.qdrant.sparseVectorName}
                      onChange={(e) =>
                        patchQdrant({ sparseVectorName: e.target.value })
                      }
                      placeholder="sparse"
                      spellCheck={false}
                    />
                  </label>
                </div>
              </div>
            </div>

            {/* ── Embedding 配置区 ── */}
            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>Embedding 模型</div>
                  <div style={s.ahaSectionDescription}>
                    走 OpenAI 兼容 API，复用既有 LLM 配置。子进程仅做推理计算，不保存密钥到磁盘以外。
                  </div>
                </div>
                <button
                  type="button"
                  style={s.ahaGhostButton}
                  onClick={() => runTest("embedding")}
                  disabled={actionInProgress !== null || saving}
                  title="测试连接"
                >
                  <RefreshCw size={13} />
                  测试连接
                </button>
              </div>
              {embeddingTest && (
                <span
                  style={{
                    ...s.ahaFeedback,
                    color:
                      embeddingTest.status === "success"
                        ? "var(--success)"
                        : "var(--danger)",
                  }}
                >
                  {embeddingTest.status === "success" ? (
                    <Check size={12} />
                  ) : null}{" "}
                  {embeddingTest.message}
                </span>
              )}
              <div style={s.ahaGrid}>
                <label style={s.ahaField}>
                  <span style={s.ahaLabel}>接口地址</span>
                  <input
                    style={s.ahaInput}
                    value={config.embedding.baseUrl}
                    onChange={(e) => patchEmbedding({ baseUrl: e.target.value })}
                    placeholder="https://api.openai.com/v1"
                    spellCheck={false}
                  />
                </label>
                <label style={s.ahaField}>
                  <span style={s.ahaLabel}>API Key</span>
                  <PasswordInput
                    value={config.embedding.apiKey}
                    onChange={(v) => patchEmbedding({ apiKey: v })}
                    placeholder="sk-..."
                  />
                </label>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>模型名</span>
                    <input
                      style={s.ahaInput}
                      value={config.embedding.model}
                      onChange={(e) => patchEmbedding({ model: e.target.value })}
                      placeholder="text-embedding-3-small"
                      spellCheck={false}
                    />
                  </label>
                  <label style={{ ...s.ahaField, width: 120, flexShrink: 0 }}>
                    <span style={s.ahaLabel}>向量维度</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={1}
                      step={1}
                      value={config.embedding.dimension}
                      onChange={(e) =>
                        patchEmbedding({ dimension: Number(e.target.value) })
                      }
                    />
                  </label>
                </div>
              </div>
            </div>

            {/* ── Sparse / Chunking / OCR ── */}
            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>分片与稀疏向量</div>
                  <div style={s.ahaSectionDescription}>
                    父子分片用于召回小块、回填父块上下文；稀疏向量用于关键词匹配。
                  </div>
                </div>
              </div>
              <div style={s.ahaGrid}>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>稀疏供应商</span>
                    <EnumSelect
                      value={config.sparseEmbedding.provider}
                      options={SPARSE_PROVIDER_OPTIONS}
                      onValueChange={(provider) =>
                        patchSparseEmbedding({
                          provider,
                          model: SPARSE_MODEL_OPTIONS_BY_PROVIDER[provider]?.[0]?.value ?? "",
                        })
                      }
                      placeholder="fastembed"
                    />
                  </label>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>稀疏模型</span>
                    <EnumSelect
                      value={config.sparseEmbedding.model}
                      options={sparseModelOptions}
                      onValueChange={(model) => patchSparseEmbedding({ model })}
                      placeholder="Qdrant/bm25"
                    />
                  </label>
                </div>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>父块大小</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={1}
                      value={config.chunking.parentChunkSize}
                      onChange={(e) =>
                        patchChunking({ parentChunkSize: Number(e.target.value) })
                      }
                    />
                  </label>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>父块重叠</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={0}
                      value={config.chunking.parentChunkOverlap}
                      onChange={(e) =>
                        patchChunking({ parentChunkOverlap: Number(e.target.value) })
                      }
                    />
                  </label>
                </div>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>子块大小</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={1}
                      value={config.chunking.childChunkSize}
                      onChange={(e) =>
                        patchChunking({ childChunkSize: Number(e.target.value) })
                      }
                    />
                  </label>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>子块重叠</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={0}
                      value={config.chunking.childChunkOverlap}
                      onChange={(e) =>
                        patchChunking({ childChunkOverlap: Number(e.target.value) })
                      }
                    />
                  </label>
                </div>
                <label style={s.ahaField}>
                  <span style={s.ahaLabel}>分隔符（每行一个）</span>
                  <textarea
                    style={{ ...s.ahaInput, minHeight: 76, resize: "vertical" }}
                    value={config.chunking.separators.join("\n")}
                    onChange={(e) =>
                      patchChunking({ separators: e.target.value.split("\n") })
                    }
                    spellCheck={false}
                  />
                </label>
              </div>
            </div>

            <div style={s.ahaSection}>
              <div style={s.ahaSectionHeader}>
                <div>
                  <div style={s.ahaSectionTitle}>OCR</div>
                  <div style={s.ahaSectionDescription}>
                    默认处理扫描 PDF、图片文件，以及 Office 文档中的嵌入图片。
                  </div>
                </div>
                <button
                  type="button"
                  style={config.ocr.enabled ? s.ahaActiveBadge : s.ahaInactiveBadge}
                  onClick={() => patchOcr({ enabled: !config.ocr.enabled })}
                >
                  {config.ocr.enabled ? "已启用" : "已关闭"}
                </button>
              </div>
              <div style={s.ahaGrid}>
                <div style={s.ragFieldRow}>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>PDF 图片宽度阈值</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={0}
                      max={1}
                      step={0.05}
                      value={config.ocr.pdfImageWidthRatio}
                      onChange={(e) =>
                        patchOcr({ pdfImageWidthRatio: Number(e.target.value) })
                      }
                    />
                  </label>
                  <label style={{ ...s.ahaField, flex: 1, minWidth: 0 }}>
                    <span style={s.ahaLabel}>PDF 图片高度阈值</span>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={0}
                      max={1}
                      step={0.05}
                      value={config.ocr.pdfImageHeightRatio}
                      onChange={(e) =>
                        patchOcr({ pdfImageHeightRatio: Number(e.target.value) })
                      }
                    />
                  </label>
                </div>
                <label style={{ ...s.ahaField, flexDirection: "row", alignItems: "center", gap: 8 }}>
                  <input
                    type="checkbox"
                    checked={config.ocr.useCuda}
                    onChange={(e) => patchOcr({ useCuda: e.target.checked })}
                  />
                  <span style={s.ahaLabel}>使用 CUDA OCR（需要本机环境支持）</span>
                </label>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* ── Footer ── */}
      <div style={s.settingsFooter}>
        {saveError && (
          <span style={{ ...s.ahaFeedback, color: "var(--danger)", marginRight: "auto" }}>
            {saveError}
          </span>
        )}
        {saved && (
          <span
            style={{
              ...s.ahaFeedback,
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
              color: "var(--success)",
              marginRight: saveError ? 12 : "auto",
            }}
          >
            <Check size={12} /> 已保存
          </span>
        )}
        <button
          type="button"
          style={{
            ...s.modalSaveBtn,
            opacity: saving || !dirty ? 0.5 : 1,
          }}
          onClick={handleSave}
          disabled={saving || !dirty}
        >
          {saving ? "保存中..." : "保存"}
        </button>
      </div>
    </>
  );
}
