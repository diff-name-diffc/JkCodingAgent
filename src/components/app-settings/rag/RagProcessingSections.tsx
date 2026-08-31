import type { RagKbConfigController } from "./useRagKbConfig";
import {
  parseBoundedNumberInput,
  SPARSE_MODEL_OPTIONS_BY_PROVIDER,
  SPARSE_PROVIDER_OPTIONS,
} from "./rag-config";
import { RagEnumSelect } from "./RagEnumSelect";

export function RagProcessingSections({ controller }: { controller: RagKbConfigController }) {
  const config = controller.config;
  if (!config) return null;
  const patchSparse = (patch: Partial<typeof config.sparseEmbedding>) =>
    controller.patchConfig("sparseEmbedding", patch);
  const patchChunking = (patch: Partial<typeof config.chunking>) =>
    controller.patchConfig("chunking", patch);
  const patchOcr = (patch: Partial<typeof config.ocr>) => controller.patchConfig("ocr", patch);
  const sparseModels =
    SPARSE_MODEL_OPTIONS_BY_PROVIDER[config.sparseEmbedding.provider] ??
    SPARSE_MODEL_OPTIONS_BY_PROVIDER.fastembed;

  return (
    <>
      <div className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">分片与稀疏向量</div>
            <div className="ai-aha-section-description">
              父子分片用于召回小块、回填父块上下文；稀疏向量用于关键词匹配。
            </div>
          </div>
        </div>
        <div className="ai-rag-grid">
          <div className="ai-rag-field-row">
            <label className="ai-settings-field-stack">
              <span className="ai-settings-field-label">稀疏供应商</span>
              <RagEnumSelect
                value={config.sparseEmbedding.provider}
                options={SPARSE_PROVIDER_OPTIONS}
                placeholder="fastembed"
                onValueChange={(provider) =>
                  patchSparse({
                    provider,
                    model: SPARSE_MODEL_OPTIONS_BY_PROVIDER[provider]?.[0]?.value ?? "",
                  })
                }
              />
            </label>
            <label className="ai-settings-field-stack">
              <span className="ai-settings-field-label">稀疏模型</span>
              <RagEnumSelect
                value={config.sparseEmbedding.model}
                options={sparseModels}
                onValueChange={(model) => patchSparse({ model })}
                placeholder="Qdrant/bm25"
              />
            </label>
          </div>
          <ChunkSizeRow
            firstLabel="父块大小"
            firstValue={config.chunking.parentChunkSize}
            secondLabel="父块重叠"
            secondValue={config.chunking.parentChunkOverlap}
            onFirst={(parentChunkSize) => patchChunking({ parentChunkSize })}
            onSecond={(parentChunkOverlap) => patchChunking({ parentChunkOverlap })}
          />
          <ChunkSizeRow
            firstLabel="子块大小"
            firstValue={config.chunking.childChunkSize}
            secondLabel="子块重叠"
            secondValue={config.chunking.childChunkOverlap}
            onFirst={(childChunkSize) => patchChunking({ childChunkSize })}
            onSecond={(childChunkOverlap) => patchChunking({ childChunkOverlap })}
          />
          <label className="ai-settings-field-stack">
            <span className="ai-settings-field-label">分隔符（每行一个）</span>
            <textarea
              className="ai-settings-textarea ai-rag-separators"
              value={config.chunking.separators.join("\n")}
              onChange={(event) => patchChunking({ separators: event.target.value.split("\n") })}
              spellCheck={false}
            />
          </label>
        </div>
      </div>

      <div className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">OCR</div>
            <div className="ai-aha-section-description">
              默认处理扫描 PDF、图片文件，以及 Office 文档中的嵌入图片。
            </div>
          </div>
          <button
            type="button"
            className={config.ocr.enabled ? "ai-rag-level-button is-active" : "ai-rag-level-button"}
            onClick={() => patchOcr({ enabled: !config.ocr.enabled })}
          >
            {config.ocr.enabled ? "已启用" : "已关闭"}
          </button>
        </div>
        <div className="ai-rag-grid">
          <div className="ai-rag-field-row">
            <RatioField
              label="PDF 图片宽度阈值"
              value={config.ocr.pdfImageWidthRatio}
              onChange={(pdfImageWidthRatio) => patchOcr({ pdfImageWidthRatio })}
            />
            <RatioField
              label="PDF 图片高度阈值"
              value={config.ocr.pdfImageHeightRatio}
              onChange={(pdfImageHeightRatio) => patchOcr({ pdfImageHeightRatio })}
            />
          </div>
          <label className="ai-rag-checkbox-field">
            <input
              type="checkbox"
              checked={config.ocr.useCuda}
              onChange={(event) => patchOcr({ useCuda: event.target.checked })}
            />
            <span className="ai-settings-field-label">使用 CUDA OCR（需要本机环境支持）</span>
          </label>
        </div>
      </div>
    </>
  );
}

function ChunkSizeRow({
  firstLabel,
  firstValue,
  secondLabel,
  secondValue,
  onFirst,
  onSecond,
}: {
  firstLabel: string;
  firstValue: number;
  secondLabel: string;
  secondValue: number;
  onFirst: (value: number) => void;
  onSecond: (value: number) => void;
}) {
  return (
    <div className="ai-rag-field-row">
      <IntegerField label={firstLabel} value={firstValue} min={1} onChange={onFirst} />
      <IntegerField label={secondLabel} value={secondValue} min={0} onChange={onSecond} />
    </div>
  );
}

function IntegerField({
  label,
  value,
  min,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="ai-settings-field-stack">
      <span className="ai-settings-field-label">{label}</span>
      <input
        className="ai-settings-input"
        type="number"
        min={min}
        value={value}
        onChange={(event) => {
          const next = parseBoundedNumberInput(event.target.value, min);
          if (next !== null) onChange(next);
        }}
      />
    </label>
  );
}

function RatioField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="ai-settings-field-stack">
      <span className="ai-settings-field-label">{label}</span>
      <input
        className="ai-settings-input"
        type="number"
        min={0}
        max={1}
        step={0.05}
        value={value}
        onChange={(event) => {
          const next = parseBoundedNumberInput(event.target.value, 0, 1);
          if (next !== null) onChange(next);
        }}
      />
    </label>
  );
}
