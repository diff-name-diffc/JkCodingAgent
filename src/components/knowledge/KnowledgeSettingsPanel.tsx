import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Save, Zap } from "lucide-react";
import type { KnowledgeModelConfig, KnowledgeSettings } from "../../types";
import s from "../../styles";

const emptyModel: KnowledgeModelConfig = { url: "", apiKey: "", model: "" };
type ModelKind = "text" | "vision" | "embedding";
type TestFeedback = { status: "success" | "error"; message: string };

const emptyTestFeedback: Record<ModelKind, TestFeedback | null> = {
  text: null,
  vision: null,
  embedding: null,
};

export function defaultKnowledgeSettings(): KnowledgeSettings {
  return {
    textModel: { ...emptyModel },
    visionModel: { ...emptyModel },
    embeddingModel: { ...emptyModel },
  };
}

export function KnowledgeSettingsPanel({
  settings,
  onSettingsSaved,
}: {
  settings: KnowledgeSettings;
  onSettingsSaved: (settings: KnowledgeSettings) => void;
}) {
  const [draft, setDraft] = useState(settings);
  const [saving, setSaving] = useState(false);
  const [testingKind, setTestingKind] = useState<ModelKind | null>(null);
  const [testFeedback, setTestFeedback] = useState<Record<ModelKind, TestFeedback | null>>(emptyTestFeedback);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);

  useEffect(() => {
    setDraft(settings);
  }, [settings]);

  function updateModel(kind: keyof KnowledgeSettings, patch: Partial<KnowledgeModelConfig>) {
    setDraft((prev) => ({ ...prev, [kind]: { ...prev[kind], ...patch } }));
  }

  async function save() {
    setSaving(true);
    setSaveMessage(null);
    try {
      const saved = await invoke<KnowledgeSettings>("knowledge_save_settings", { settings: draft });
      onSettingsSaved(saved);
      setSaveMessage("配置已保存");
    } catch (error) {
      setSaveMessage(String(error));
    } finally {
      setSaving(false);
    }
  }

  async function test(kind: ModelKind) {
    if (testingKind) return;
    setTestingKind(kind);
    setSaveMessage(null);
    setTestFeedback((prev) => ({ ...prev, [kind]: null }));
    try {
      const message = await invoke<string>("knowledge_test_model", { kind, settings: draft });
      setTestFeedback((prev) => ({ ...prev, [kind]: { status: "success", message } }));
    } catch (error) {
      setTestFeedback((prev) => ({ ...prev, [kind]: { status: "error", message: String(error) } }));
    } finally {
      setTestingKind(null);
    }
  }

  return (
    <div style={s.knowledgeForm}>
      <ModelSection
        title="文本模型"
        description="用于源文件解析、页面生成和页面合并。OpenAI-compatible chat completions。"
        value={draft.textModel}
        onChange={(patch) => updateModel("textModel", patch)}
        onTest={() => test("text")}
        testing={testingKind === "text"}
        disabled={testingKind !== null || saving}
        feedback={testFeedback.text}
      />
      <ModelSection
        title="多模态模型"
        description="用于 PDF、Office 和图片源文件中的图片说明；留空时只保留图片引用。"
        value={draft.visionModel}
        onChange={(patch) => updateModel("visionModel", patch)}
        onTest={() => test("vision")}
        testing={testingKind === "vision"}
        disabled={testingKind !== null || saving}
        feedback={testFeedback.vision}
      />
      <ModelSection
        title="Embedding 模型"
        description="用于向量索引和知识库混合检索。URL 可填 base URL 或 /v1/embeddings。"
        value={draft.embeddingModel}
        onChange={(patch) => updateModel("embeddingModel", patch)}
        onTest={() => test("embedding")}
        testing={testingKind === "embedding"}
        disabled={testingKind !== null || saving}
        feedback={testFeedback.embedding}
      />
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <button style={s.knowledgeSmallBtn} type="button" onClick={save} disabled={saving || testingKind !== null}>
          {saving ? <Zap size={14} /> : <Save size={14} />}
          保存配置
        </button>
        {saveMessage && (
          <span
            style={{ display: "inline-flex", alignItems: "center", gap: 6, color: "var(--text-muted)", fontSize: 12 }}
            aria-live="polite"
          >
            <Check size={13} />
            {saveMessage}
          </span>
        )}
      </div>
    </div>
  );
}

function ModelSection({
  title,
  description,
  value,
  onChange,
  onTest,
  testing,
  disabled,
  feedback,
}: {
  title: string;
  description: string;
  value: KnowledgeModelConfig;
  onChange: (patch: Partial<KnowledgeModelConfig>) => void;
  onTest: () => void;
  testing: boolean;
  disabled: boolean;
  feedback: TestFeedback | null;
}) {
  return (
    <section style={s.knowledgeCard}>
      <div style={{ fontSize: 14, fontWeight: 750, color: "var(--text-primary)" }}>{title}</div>
      <div style={{ marginTop: 5, marginBottom: 14, fontSize: 12, color: "var(--text-muted)", lineHeight: 1.5 }}>
        {description}
      </div>
      <div style={{ display: "grid", gap: 12 }}>
        <label style={s.knowledgeField}>
          <span style={s.knowledgeLabel}>URL</span>
          <input
            style={s.knowledgeInput}
            value={value.url}
            placeholder="https://api.example.com/v1"
            onChange={(event) => onChange({ url: event.target.value })}
          />
        </label>
        <label style={s.knowledgeField}>
          <span style={s.knowledgeLabel}>API Key</span>
          <input
            style={s.knowledgeInput}
            value={value.apiKey}
            type="password"
            placeholder="sk-..."
            onChange={(event) => onChange({ apiKey: event.target.value })}
          />
        </label>
        <label style={s.knowledgeField}>
          <span style={s.knowledgeLabel}>Model</span>
          <input
            style={s.knowledgeInput}
            value={value.model}
            placeholder="model-name"
            onChange={(event) => onChange({ model: event.target.value })}
          />
        </label>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginTop: 12 }}>
        <button style={s.knowledgeSmallBtn} type="button" onClick={onTest} disabled={disabled}>
          <Zap size={14} />
          {testing ? "测试中..." : "测试"}
        </button>
        {testing ? (
          <span style={{ color: "var(--text-muted)", fontSize: 12 }} aria-live="polite">
            测试中...
          </span>
        ) : feedback ? (
          <span
            style={{ color: feedback.status === "success" ? "var(--success)" : "var(--danger)", fontSize: 12 }}
            aria-live="polite"
          >
            {feedback.message}
          </span>
        ) : null}
      </div>
    </section>
  );
}
