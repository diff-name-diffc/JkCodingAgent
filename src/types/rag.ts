// 字段名严格对齐 src-tauri/src/rag/config.rs 的 #[serde(rename_all = "camelCase")]。
// 修改任一字段必须同步 Rust struct 与 rag/src/rag_server/config.py。

export interface RagQdrantConfig {
  url: string;
  apiKey: string;
  collectionPrefix: string;
  timeout: number;
  denseVectorName: string;
  sparseVectorName: string;
}

export interface RagEmbeddingConfig {
  provider: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  dimension: number;
}

export interface RagSparseEmbeddingConfig {
  provider: string;
  model: string;
}

export interface RagChunkingConfig {
  parentChunkSize: number;
  parentChunkOverlap: number;
  childChunkSize: number;
  childChunkOverlap: number;
  separators: string[];
}

export interface RagOcrConfig {
  enabled: boolean;
  useCuda: boolean;
  pdfImageWidthRatio: number;
  pdfImageHeightRatio: number;
}

export interface RagKbConfig {
  qdrant: RagQdrantConfig;
  embedding: RagEmbeddingConfig;
  sparseEmbedding: RagSparseEmbeddingConfig;
  chunking: RagChunkingConfig;
  ocr: RagOcrConfig;
  logLevel: string;
}

export interface RagKbSaveResult {
  saved: boolean;
  reloaded: boolean;
  reloadError: string | null;
}

export interface RagRuntimeStatus {
  running: boolean;
  port?: number | null;
}

export interface RagIngestJobStartResult {
  jobId: string;
}

export type RagIngestFileStatus = "pending" | "running" | "done" | "failed";
export type RagIngestJobStatusType = "queued" | "running" | "done" | "partial" | "failed";

export interface RagIngestFileResult {
  path: string;
  status: RagIngestFileStatus;
  rawDocuments: number;
  parentChunks: number;
  childChunks: number;
  indexedPoints: number;
  error?: string | null;
}

export interface RagIngestJobStatus {
  jobId: string;
  projectId: string;
  status: RagIngestJobStatusType;
  totalFiles: number;
  completedFiles: number;
  failedFiles: number;
  createdAt: number;
  updatedAt: number;
  error?: string | null;
  files: RagIngestFileResult[];
}

export type RagLogStream = "stdout" | "stderr" | "system";
export type RagLogLevel = "debug" | "info" | "warn" | "error" | "system";

export interface RagLogEntry {
  seq: number;
  ts: number;
  stream: RagLogStream;
  level?: RagLogLevel;
  text: string;
}
