import { Check, RefreshCw } from "lucide-react";
import type { RagKbConfigController, RagTestFeedback } from "./useRagKbConfig";
import { PasswordInput } from "./PasswordInput";
import { parseBoundedNumberInput } from "./rag-config";

export function RagVectorSections({ controller }: { controller: RagKbConfigController }) {
  const config = controller.config;
  if (!config) return null;
  const patchQdrant = (patch: Partial<typeof config.qdrant>) =>
    controller.patchConfig("qdrant", patch);
  const patchEmbedding = (patch: Partial<typeof config.embedding>) =>
    controller.patchConfig("embedding", patch);
  return (
    <>
      <div className="ai-aha-section">
        <VectorSectionHeader
          title="Qdrant 向量库"
          description="外部独立部署的 Qdrant 实例连接信息。配置存于本地，启动时注入子进程。"
          feedback={controller.qdrantTest}
          disabled={controller.actionInProgress !== null || controller.saving}
          onTest={() => controller.runTest("qdrant")}
        />
        <div className="ai-rag-grid">
          <label className="ai-settings-field-stack">
            <span className="ai-settings-field-label">HTTP 端点</span>
            <input
              className="ai-settings-input"
              value={config.qdrant.url}
              onChange={(event) => patchQdrant({ url: event.target.value })}
              placeholder="http://127.0.0.1:6333"
              spellCheck={false}
            />
            <span className="ai-settings-hint">Qdrant 的 HTTP REST 端点地址。</span>
          </label>
          <label className="ai-settings-field-stack">
            <span className="ai-settings-field-label">API Key</span>
            <PasswordInput
              value={config.qdrant.apiKey}
              onChange={(apiKey) => patchQdrant({ apiKey })}
              placeholder="可选，留空则不鉴权"
            />
          </label>
          <div className="ai-rag-field-row">
            <TextField
              label="命名前缀"
              value={config.qdrant.collectionPrefix}
              onChange={(collectionPrefix) => patchQdrant({ collectionPrefix })}
              placeholder="jk_"
            />
            <NumberField
              label="超时（秒）"
              value={config.qdrant.timeout}
              min={1}
              compact
              onChange={(timeout) => patchQdrant({ timeout })}
            />
          </div>
          <div className="ai-rag-field-row">
            <TextField
              label="稠密向量名"
              value={config.qdrant.denseVectorName}
              onChange={(denseVectorName) => patchQdrant({ denseVectorName })}
              placeholder="dense"
            />
            <TextField
              label="稀疏向量名"
              value={config.qdrant.sparseVectorName}
              onChange={(sparseVectorName) => patchQdrant({ sparseVectorName })}
              placeholder="sparse"
            />
          </div>
        </div>
      </div>

      <div className="ai-aha-section">
        <VectorSectionHeader
          title="Embedding 模型"
          description="走 OpenAI 兼容 API，复用既有 LLM 配置。子进程仅做推理计算，不保存密钥到磁盘以外。"
          feedback={controller.embeddingTest}
          disabled={controller.actionInProgress !== null || controller.saving}
          onTest={() => controller.runTest("embedding")}
        />
        <div className="ai-rag-grid">
          <TextField
            label="接口地址"
            value={config.embedding.baseUrl}
            onChange={(baseUrl) => patchEmbedding({ baseUrl })}
            placeholder="https://api.openai.com/v1"
          />
          <label className="ai-settings-field-stack">
            <span className="ai-settings-field-label">API Key</span>
            <PasswordInput
              value={config.embedding.apiKey}
              onChange={(apiKey) => patchEmbedding({ apiKey })}
              placeholder="sk-..."
            />
          </label>
          <div className="ai-rag-field-row">
            <TextField
              label="模型名"
              value={config.embedding.model}
              onChange={(model) => patchEmbedding({ model })}
              placeholder="text-embedding-3-small"
            />
            <NumberField
              label="向量维度"
              value={config.embedding.dimension}
              min={1}
              compact
              onChange={(dimension) => patchEmbedding({ dimension })}
            />
          </div>
        </div>
      </div>
    </>
  );
}

function VectorSectionHeader({
  title,
  description,
  feedback,
  disabled,
  onTest,
}: {
  title: string;
  description: string;
  feedback: RagTestFeedback | null;
  disabled: boolean;
  onTest: () => void;
}) {
  return (
    <>
      <div className="ai-aha-section-header">
        <div>
          <div className="ai-aha-section-title">{title}</div>
          <div className="ai-aha-section-description">{description}</div>
        </div>
        <button
          type="button"
          className="ai-aha-ghost-button"
          onClick={onTest}
          disabled={disabled}
          title="测试连接"
        >
          <RefreshCw size={13} />
          测试连接
        </button>
      </div>
      {feedback && (
        <span
          className={
            feedback.status === "success"
              ? "ai-rag-feedback is-success"
              : "ai-rag-feedback is-error"
          }
        >
          {feedback.status === "success" && <Check size={12} />} {feedback.message}
        </span>
      )}
    </>
  );
}

function TextField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <label className="ai-settings-field-stack">
      <span className="ai-settings-field-label">{label}</span>
      <input
        className="ai-settings-input"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        spellCheck={false}
      />
    </label>
  );
}

function NumberField({
  label,
  value,
  min,
  compact,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  compact?: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <label className={`ai-settings-field-stack${compact ? " ai-rag-field-compact" : ""}`}>
      <span className="ai-settings-field-label">{label}</span>
      <input
        className="ai-settings-input"
        type="number"
        min={min}
        step={1}
        value={value}
        onChange={(event) => {
          const next = parseBoundedNumberInput(event.target.value, min);
          if (next !== null) onChange(next);
        }}
      />
    </label>
  );
}
