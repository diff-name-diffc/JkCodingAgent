import type { RagKbConfig } from "../../../types";

export const LOG_LEVELS = [
  { value: "DEBUG", label: "Debug" },
  { value: "INFO", label: "Info" },
  { value: "WARNING", label: "Warning" },
  { value: "ERROR", label: "Error" },
];

export const RAG_FILE_EXTENSIONS = [
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

export const SPARSE_PROVIDER_OPTIONS = [{ value: "fastembed", label: "FastEmbed" }];

export const SPARSE_MODEL_OPTIONS_BY_PROVIDER: Record<
  string,
  Array<{ value: string; label: string }>
> = {
  fastembed: [
    { value: "Qdrant/bm25", label: "Qdrant/bm25" },
    { value: "Qdrant/minicoil-v1", label: "Qdrant/minicoil-v1" },
    {
      value: "Qdrant/bm42-all-minilm-l6-v2-attentions",
      label: "Qdrant/bm42-all-minilm-l6-v2-attentions",
    },
    { value: "prithivida/Splade_PP_en_v1", label: "prithivida/Splade_PP_en_v1" },
  ],
};

export function normalizeLogLevel(value: string): string {
  const normalized = value.trim().toUpperCase();
  return LOG_LEVELS.some((item) => item.value === normalized) ? normalized : "INFO";
}

export function parseBoundedNumberInput(
  raw: string,
  min: number,
  max = Number.POSITIVE_INFINITY,
): number | null {
  if (!raw.trim()) return null;
  const value = Number(raw);
  return Number.isFinite(value) && value >= min && value <= max ? value : null;
}

export function normalizeSparseConfig(config: RagKbConfig): RagKbConfig {
  const provider = SPARSE_PROVIDER_OPTIONS.some(
    (option) => option.value === config.sparseEmbedding.provider,
  )
    ? config.sparseEmbedding.provider
    : SPARSE_PROVIDER_OPTIONS[0].value;
  const options = SPARSE_MODEL_OPTIONS_BY_PROVIDER[provider] ?? [];
  // 未知/非法模型名统一回落到该 provider 的首个可选项。
  return {
    ...config,
    sparseEmbedding: {
      provider,
      model: options.some((option) => option.value === config.sparseEmbedding.model)
        ? config.sparseEmbedding.model
        : (options[0]?.value ?? config.sparseEmbedding.model),
    },
  };
}

export function ragFileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

export type RagRuntimeState = "running" | "starting" | "stopped";

/**
 * RAG 运行态派生（UI-22c）：区分「已运行 / 启动中（探活窗口内或重启中）/
 * 未运行（探活窗口耗尽仍未起）」——不再把启动失败或未响应永久显示成「启动中…」，
 * 收敛「RAG 错误被通用态吞掉」。纯函数，只产出语义态与文案；配色类名由调用方
 * 映射，真实失败原因由服务日志面板承担。
 */
export function deriveRagRuntimeState(input: {
  running: boolean;
  restarting: boolean;
  probing: boolean;
  port?: number | null;
}): { state: RagRuntimeState; text: string } {
  if (input.running) {
    return { state: "running", text: `已运行 · 端口 ${input.port ?? "-"}` };
  }
  if (input.restarting) {
    return { state: "starting", text: "重启中…" };
  }
  if (input.probing) {
    return { state: "starting", text: "启动中…" };
  }
  return { state: "stopped", text: "未运行" };
}
