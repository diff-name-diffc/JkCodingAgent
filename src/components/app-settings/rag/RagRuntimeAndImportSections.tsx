import { FileText, RotateCw, Upload, X } from "lucide-react";
import type { RagKbConfigController } from "./useRagKbConfig";
import { LOG_LEVELS, normalizeLogLevel, ragFileName } from "./rag-config";
import { RagSidecarLogPanel } from "./RagSidecarLogPanel";

export function RagRuntimeAndImportSections({ controller }: { controller: RagKbConfigController }) {
  const config = controller.config;
  if (!config) return null;
  const running = controller.runtimeStatus.running;
  return (
    <>
      <div className="ai-rag-runtime-bar">
        <div className="ai-rag-runtime-info">
          <span className={running ? "ai-rag-status-dot is-running" : "ai-rag-status-dot"} />
          <span className="ai-rag-status-text">
            {running ? `已运行 · 端口 ${controller.runtimeStatus.port ?? "-"}` : "启动中…"}
          </span>
        </div>
        <div className="ai-aha-action-row">
          <button
            type="button"
            className="ai-aha-ghost-button"
            onClick={controller.restart}
            disabled={controller.actionInProgress !== null}
            title="重启 RAG 服务"
          >
            <RotateCw size={13} />
            {controller.actionInProgress === "restart" ? "重启中..." : "重启"}
          </button>
        </div>
      </div>

      <div className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">服务日志</div>
            <div className="ai-aha-section-description">
              日志只保留内存中的最近 2000 行。等级保存后会热更新到运行中的 sidecar。
            </div>
          </div>
        </div>
        <div className="ai-settings-field-stack">
          <span className="ai-settings-field-label">日志等级</span>
          <div className="ai-aha-action-row">
            {LOG_LEVELS.map((level) => (
              <button
                key={level.value}
                type="button"
                className={
                  normalizeLogLevel(config.logLevel) === level.value
                    ? "ai-rag-level-button is-active"
                    : "ai-rag-level-button"
                }
                onClick={() =>
                  controller.setConfig((previous) =>
                    previous ? { ...previous, logLevel: level.value } : previous,
                  )
                }
              >
                {level.label}
              </button>
            ))}
          </div>
          <span className="ai-settings-hint">
            Debug 适合排查启动和连接问题；日常建议保持 Info 或 Warning。
          </span>
        </div>
        <RagSidecarLogPanel />
      </div>

      <div className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">导入文档</div>
            <div className="ai-aha-section-description">
              支持 PDF、Office、Markdown、文本、表格、HTML 与图片；文件必须位于当前项目目录内。
            </div>
          </div>
          <div className="ai-aha-action-row">
            <button
              type="button"
              className="ai-aha-ghost-button"
              onClick={controller.pickFiles}
              disabled={!controller.importReady || controller.ingesting}
              title="选择文件"
            >
              <FileText size={13} />
              选择
            </button>
            <button
              type="button"
              className="ai-aha-ghost-button"
              onClick={controller.ingest}
              disabled={
                !controller.selectedFiles.length || !controller.importReady || controller.ingesting
              }
              title="导入知识库"
            >
              <Upload size={13} />
              {controller.ingesting ? "导入中..." : "导入"}
            </button>
          </div>
        </div>
        {!controller.importReady && (
          <span className="ai-settings-hint">请从具体项目打开设置后再导入文档。</span>
        )}
        {controller.selectedFiles.length > 0 && (
          <div className="ai-rag-selected-files">
            {controller.selectedFiles.map((file) => (
              <div key={file} className="ai-rag-selected-file">
                <FileText size={13} />
                <span className="ai-rag-selected-file-name">{ragFileName(file)}</span>
                <button
                  type="button"
                  className="ai-rag-icon-button"
                  onClick={() => controller.removeFile(file)}
                  disabled={controller.ingesting}
                  title="移除"
                >
                  <X size={12} />
                </button>
              </div>
            ))}
          </div>
        )}
        {controller.ingestError && (
          <span className="ai-rag-feedback is-error">{controller.ingestError}</span>
        )}
        {controller.ingestJob && (
          <div className="ai-rag-ingest-status">
            <span className="ai-settings-hint">
              状态：{controller.ingestJob.status} · {controller.ingestJob.completedFiles}/
              {controller.ingestJob.totalFiles} 完成
              {controller.ingestJob.failedFiles > 0
                ? ` · ${controller.ingestJob.failedFiles} 失败`
                : ""}
            </span>
            {controller.ingestJob.files.map((file) => (
              <span
                key={file.path}
                className={
                  file.status === "failed" ? "ai-rag-ingest-file is-failed" : "ai-rag-ingest-file"
                }
              >
                {ragFileName(file.path)} · {file.status}
                {file.status === "done"
                  ? ` · ${file.parentChunks} parent / ${file.indexedPoints} vectors`
                  : ""}
                {file.error ? ` · ${file.error}` : ""}
              </span>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
