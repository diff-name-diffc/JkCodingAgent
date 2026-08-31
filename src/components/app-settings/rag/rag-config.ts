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
