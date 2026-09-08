import { useEffect, useRef } from "react";
import { useToast } from "../../Toast";
import { publishSaveSource, registerSaveSource } from "../../settings/save-sources";
import { RagProcessingSections } from "./RagProcessingSections";
import { RagRuntimeAndImportSections } from "./RagRuntimeAndImportSections";
import { RagVectorSections } from "./RagVectorSections";
import { useRagKbConfig } from "./useRagKbConfig";

interface RagKbConfigPanelProps {
  /** 与 AhaAgentPanel 对齐，预留按项目隔离扩展（当前 RAG 配置为全局）。 */
  projectId?: string;
  projectPath?: string;
}

/** RAG 配置在统一保存状态注册表中的源 id（UI-21 遗留领取）。 */
const RAG_SAVE_SOURCE_ID = "rag-kb-config";

export function RagKbConfigPanel({ projectId, projectPath }: RagKbConfigPanelProps) {
  const { showToast } = useToast();
  const controller = useRagKbConfig({ projectId, projectPath, showToast });

  // 统一保存状态发布：RAG 为手动保存源——dirty 语义是「未保存的修改」而非
  // 「保存中」，聚合层按 mode 区分显示。controller.save 身份随 config 变化，
  // 经 ref 转发避免注册闭包过期。
  const saveRef = useRef(controller.save);
  saveRef.current = controller.save;
  useEffect(() => {
    publishSaveSource(RAG_SAVE_SOURCE_ID, {
      mode: "manual",
      dirty: controller.dirty,
      saving: controller.saving,
      hasError: controller.saveError != null,
    });
  }, [controller.dirty, controller.saving, controller.saveError]);
  // 卸载不 flush：切导航页放弃未保存修改是 RAG 手动保存模型的既有语义，
  // 仅关闭弹窗时经确认框「保存并关闭」走 flushAllSaveSources。
  useEffect(() => registerSaveSource(RAG_SAVE_SOURCE_ID, () => saveRef.current()), []);

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
