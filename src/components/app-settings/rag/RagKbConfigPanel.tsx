import { Check } from "lucide-react";
import { useToast } from "../../Toast";
import { RagProcessingSections } from "./RagProcessingSections";
import { RagRuntimeAndImportSections } from "./RagRuntimeAndImportSections";
import { RagVectorSections } from "./RagVectorSections";
import { useRagKbConfig } from "./useRagKbConfig";

interface RagKbConfigPanelProps {
  /** 与 AhaAgentPanel 对齐，预留按项目隔离扩展（当前 RAG 配置为全局）。 */
  projectId?: string;
  projectPath?: string;
}

export function RagKbConfigPanel({ projectId, projectPath }: RagKbConfigPanelProps) {
  const { showToast } = useToast();
  const controller = useRagKbConfig({ projectId, projectPath, showToast });

  if (controller.loading || !controller.config) {
    return (
      <div className="ai-rag-panel">
        <div className="ai-settings-empty">加载中...</div>
      </div>
    );
  }

  return (
    <>
      <div className="ai-rag-panel">
        <div className="ai-rag-body chat-scroll">
          <div className="ai-rag-content">
            <RagRuntimeAndImportSections controller={controller} />
            <RagVectorSections controller={controller} />
            <RagProcessingSections controller={controller} />
          </div>
        </div>
      </div>
      <div className="ai-settings-footer ai-rag-footer">
        {controller.saveError && (
          <span className="ai-rag-feedback is-error">{controller.saveError}</span>
        )}
        {controller.saved && (
          <span className="ai-rag-feedback is-success">
            <Check size={12} /> 已保存
          </span>
        )}
        <button
          type="button"
          className="ai-primary-button"
          onClick={controller.save}
          disabled={controller.saving || !controller.dirty}
        >
          {controller.saving ? "保存中..." : "保存"}
        </button>
      </div>
    </>
  );
}
