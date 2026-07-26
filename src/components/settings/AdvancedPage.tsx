import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Pencil, RefreshCw } from "lucide-react";
import { highlightCodeToHtml } from "../../utils/shiki";
import { Section } from "./Section";
import claudeLogo from "../../assets/claude.svg";
import chatgptLogo from "../../assets/chatgpt.svg";

type AgentKey = "claude" | "codex";

interface AppSettings {
  claude_path: string;
  codex_path: string;
}

interface AgentVersions {
  claude_version: string;
  codex_version: string;
}

type FileState =
  | { status: "loading" }
  | { status: "missing" }
  | { status: "loaded"; content: string };

/**
 * 「高级」页：Claude Code / Codex 的安装版本与配置文件编辑（合并为一页两分区）。
 * 编辑态通过 reportDirty 上报，关闭弹窗前提示。
 */
export function AdvancedPage({ reportDirty }: { reportDirty: (dirty: boolean) => void }) {
  const [dirtyKeys, setDirtyKeys] = useState<Set<AgentKey>>(new Set());

  useEffect(() => {
    reportDirty(dirtyKeys.size > 0);
  }, [dirtyKeys, reportDirty]);

  // 稳定引用 + 无变化时返回原集合，避免子组件 effect 反复触发造成多余渲染。
  const setKeyDirty = useCallback((key: AgentKey, dirty: boolean) => {
    setDirtyKeys((prev) => {
      if (prev.has(key) === dirty) return prev;
      const next = new Set(prev);
      if (dirty) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);

  return (
    <div className="ai-set-page">
      <Section title="Claude Code" description="配置文件 ~/.claude/settings.json">
        <AgentConfigEditor
          agentKey="claude"
          logo={claudeLogo}
          lang="json"
          onDirtyChange={(dirty) => setKeyDirty("claude", dirty)}
        />
      </Section>
      <Section title="Codex" description="配置文件 ~/.codex/config.toml">
        <AgentConfigEditor
          agentKey="codex"
          logo={chatgptLogo}
          lang="toml"
          onDirtyChange={(dirty) => setKeyDirty("codex", dirty)}
        />
      </Section>
    </div>
  );
}

function AgentConfigEditor({
  agentKey,
  logo,
  lang,
  onDirtyChange,
}: {
  agentKey: AgentKey;
  logo: string;
  lang: string;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const [fileState, setFileState] = useState<FileState>({ status: "loading" });
  const [original, setOriginal] = useState("");
  const [editing, setEditing] = useState(false);
  const [highlighted, setHighlighted] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [detectedVersion, setDetectedVersion] = useState("");
  const [versionSourcePath, setVersionSourcePath] = useState("");
  const [versionLoading, setVersionLoading] = useState(true);
  const [detectingVersion, setDetectingVersion] = useState(false);
  const [versionError, setVersionError] = useState<string | null>(null);

  const isDirty = fileState.status === "loaded" && fileState.content !== original;

  useEffect(() => {
    onDirtyChange(editing && isDirty);
  }, [editing, isDirty, onDirtyChange]);

  const refreshAgentVersion = useCallback(
    async (showLoadingState: boolean) => {
      if (showLoadingState) {
        setVersionLoading(true);
      } else {
        setDetectingVersion(true);
      }
      setVersionError(null);
      try {
        const settings = await invoke<AppSettings>("load_app_settings");
        const configuredPath = agentKey === "claude" ? settings.claude_path : settings.codex_path;
        setVersionSourcePath(configuredPath);
        const versions = await invoke<AgentVersions>("detect_agent_versions_for_settings", {
          settings,
        });
        setDetectedVersion(
          agentKey === "claude" ? versions.claude_version : versions.codex_version,
        );
      } catch (e) {
        setVersionError(String(e));
      } finally {
        if (showLoadingState) {
          setVersionLoading(false);
        } else {
          setDetectingVersion(false);
        }
      }
    },
    [agentKey],
  );

  useEffect(() => {
    setFileState({ status: "loading" });
    setEditing(false);
    setHighlighted(null);
    setError(null);
    setSaved(false);
    invoke<string | null>("read_agent_config_file", { agent: agentKey })
      .then((content) => {
        if (content === null) {
          setFileState({ status: "missing" });
        } else {
          setFileState({ status: "loaded", content });
          setOriginal(content);
        }
      })
      .catch((e) => setError(String(e)));
  }, [agentKey]);

  useEffect(() => {
    void refreshAgentVersion(true);
  }, [refreshAgentVersion]);

  useEffect(() => {
    if (fileState.status !== "loaded") return;
    let cancelled = false;
    setHighlighted(null);
    highlightCodeToHtml(fileState.content, lang).then((html) => {
      if (!cancelled) setHighlighted(html);
    });
    return () => {
      cancelled = true;
    };
  }, [fileState, lang]);

  async function handleSave() {
    if (fileState.status !== "loaded") return;
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      await invoke("write_agent_config_file", { agent: agentKey, content: fileState.content });
      setOriginal(fileState.content);
      setSaved(true);
      setEditing(false);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  function handleCancel() {
    setFileState({ status: "loaded", content: original });
    setEditing(false);
  }

  return (
    <div className="ai-set-agent-config">
      <div className="ai-settings-version-card">
        <div className="ai-settings-section-head">
          <div className="ai-settings-title-stack">
            <span className="ai-settings-section-title">
              <img src={logo} alt="" width={14} height={14} className="ai-settings-logo" />
              已安装版本
            </span>
            <span className="ai-settings-hint">
              {versionSourcePath
                ? `当前可执行文件：${versionSourcePath}`
                : "当前使用系统 PATH 中的可执行文件。"}
            </span>
          </div>
          <button
            className="ai-settings-tool-button"
            onClick={() => void refreshAgentVersion(false)}
            disabled={detectingVersion}
            type="button"
          >
            <RefreshCw size={12} className={detectingVersion ? "spin" : undefined} />
            {detectingVersion ? "检测中..." : "检测"}
          </button>
        </div>
        <input
          className="ai-settings-input"
          value={detectedVersion}
          readOnly
          placeholder={versionLoading ? "检测中..." : "未检测到"}
          spellCheck={false}
        />
        {versionError && <div className="ai-settings-error">{versionError}</div>}
      </div>

      <div className="ai-settings-file-row">
        {fileState.status === "loaded" && !editing && (
          <button className="ai-settings-tool-button" onClick={() => setEditing(true)} type="button">
            <Pencil size={12} />
            编辑
          </button>
        )}
        {saved && (
          <span className="ai-settings-saved">
            <Check size={12} /> 已保存
          </span>
        )}
      </div>

      {error && <div className="ai-settings-error">{error}</div>}
      {fileState.status === "loading" && !error && (
        <div className="ai-settings-empty">加载中...</div>
      )}
      {fileState.status === "missing" && <div className="ai-settings-empty">未找到配置文件</div>}
      {fileState.status === "loaded" && !editing && (
        <div
          className="file-viewer-code ai-settings-code-view chat-scroll"
          dangerouslySetInnerHTML={{ __html: highlighted ?? "" }}
        />
      )}
      {fileState.status === "loaded" && editing && (
        <>
          <textarea
            autoFocus
            className="ai-settings-textarea"
            style={{ caretColor: "var(--foreground)" }}
            value={fileState.content}
            onChange={(e) => setFileState({ status: "loaded", content: e.target.value })}
            spellCheck={false}
          />
          <div className="ai-set-page-footer">
            <button className="ai-secondary-button" onClick={handleCancel} type="button">
              取消
            </button>
            <button
              className="ai-primary-button"
              onClick={handleSave}
              disabled={saving || !isDirty}
              type="button"
            >
              {saving ? "保存中..." : "保存"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
