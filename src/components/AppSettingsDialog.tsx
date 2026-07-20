import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Pencil, Check, RefreshCw, Database, Settings2, Monitor, Moon, Sun } from "lucide-react";
import { AhaAgentPanel } from "./app-settings/aha/AhaAgentPanel";
import { RagKbConfigPanel } from "./app-settings/rag/RagKbConfigPanel";
import claudeLogo from "../assets/claude.svg";
import chatgptLogo from "../assets/chatgpt.svg";
import appLogo from "../assets/app-logo.png";
import { highlightCodeToHtml } from "../utils/shiki";
import { applyThemePreference, persistThemePreference, type ThemePreference } from "../lib/theme";

type NavKey = "general" | "aha" | "rag" | "claude" | "codex";

interface AppSettings {
  claude_path: string;
  codex_path: string;
  theme: ThemePreference;
}

interface AgentVersions {
  claude_version: string;
  codex_version: string;
}

type AgentKey = "claude" | "codex";

const NAV_ITEMS: Array<{
  key: NavKey;
  label: string;
  logo?: string;
  filePath?: string;
  lang?: string;
}> = [
  { key: "general", label: "通用" },
  { key: "aha", label: "Aha 智能体", logo: appLogo },
  { key: "rag", label: "RAG 知识库" },
  {
    key: "claude",
    label: "Claude Code",
    logo: claudeLogo,
    filePath: "~/.claude/settings.json",
    lang: "json",
  },
  {
    key: "codex",
    label: "Codex",
    logo: chatgptLogo,
    filePath: "~/.codex/config.toml",
    lang: "toml",
  },
];

// ── General Panel ─────────────────────────────────────────────────────────────

function GeneralPanel() {
  const emptySettings: AppSettings = { claude_path: "", codex_path: "", theme: "system" };
  const [settings, setSettings] = useState<AppSettings>(emptySettings);
  const [original, setOriginal] = useState<AppSettings>(emptySettings);
  const [loading, setLoading] = useState(true);
  const [detectingPaths, setDetectingPaths] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const savedThemeRef = useRef<ThemePreference>("system");

  useEffect(() => {
    invoke<AppSettings>("load_app_settings")
      .then(async (loadedSettings) => {
        setSettings(loadedSettings);
        setOriginal(loadedSettings);
        savedThemeRef.current = loadedSettings.theme;
        persistThemePreference(loadedSettings.theme);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(
    () => () => {
      applyThemePreference(savedThemeRef.current);
    },
    [],
  );

  async function handleDetect() {
    setDetectingPaths(true);
    setError(null);
    try {
      const detected = await invoke<AppSettings>("detect_agent_paths");
      setSettings((current) => ({ ...detected, theme: current.theme }));
    } catch (e) {
      setError(String(e));
    } finally {
      setDetectingPaths(false);
    }
  }

  async function handleSave() {
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      await invoke("save_app_settings", { settings });
      persistThemePreference(settings.theme);
      savedThemeRef.current = settings.theme;
      setOriginal(settings);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  const isDirty =
    settings.claude_path !== original.claude_path ||
    settings.codex_path !== original.codex_path ||
    settings.theme !== original.theme;

  return (
    <>
      <div className="ai-settings-body ai-settings-general">
        {error && <div className="ai-settings-error">{error}</div>}

        {loading ? (
          <div className="ai-settings-empty">加载中...</div>
        ) : (
          <>
            <div className="ai-settings-field-stack">
              <span className="ai-settings-field-label">外观</span>
              <div className="ai-theme-options" role="radiogroup" aria-label="外观主题">
                {(
                  [
                    { value: "system", label: "跟随系统", icon: Monitor },
                    { value: "light", label: "浅色", icon: Sun },
                    { value: "dark", label: "深色", icon: Moon },
                  ] as const
                ).map(({ value, label, icon: Icon }) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={settings.theme === value}
                    className={
                      settings.theme === value ? "ai-theme-option is-active" : "ai-theme-option"
                    }
                    onClick={() => {
                      setSettings((current) => ({ ...current, theme: value }));
                      applyThemePreference(value);
                    }}
                  >
                    <Icon size={14} />
                    {label}
                  </button>
                ))}
              </div>
              <span className="ai-settings-hint">默认跟随操作系统，可随时手动覆盖。</span>
            </div>

            <div className="ai-settings-section-head">
              <span className="ai-settings-section-title">智能体安装路径</span>
              <button
                className="ai-settings-tool-button"
                onClick={handleDetect}
                disabled={detectingPaths}
                type="button"
              >
                <RefreshCw size={12} className={detectingPaths ? "spin" : undefined} />
                {detectingPaths ? "检测中..." : "自动检测"}
              </button>
            </div>

            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">Claude Code 路径</label>
              <input
                className="ai-settings-input"
                value={settings.claude_path}
                onChange={(e) => setSettings((prev) => ({ ...prev, claude_path: e.target.value }))}
                placeholder="/usr/local/bin/claude"
                spellCheck={false}
              />
              <span className="ai-settings-hint">留空则使用系统 PATH 中的 `claude`。</span>
            </div>

            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">Codex 路径</label>
              <input
                className="ai-settings-input"
                value={settings.codex_path}
                onChange={(e) => setSettings((prev) => ({ ...prev, codex_path: e.target.value }))}
                placeholder="/usr/local/bin/codex"
                spellCheck={false}
              />
              <span className="ai-settings-hint">留空则使用系统 PATH 中的 `codex`。</span>
            </div>
          </>
        )}
      </div>

      <div className="ai-settings-footer">
        {saved && (
          <span className="ai-settings-saved">
            <Check size={12} /> 已保存
          </span>
        )}
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
  );
}

// ── Agent Config Panel ────────────────────────────────────────────────────────

type FileState =
  { status: "loading" } | { status: "missing" } | { status: "loaded"; content: string };

function AgentConfigPanel({
  agentKey,
  filePath,
  lang,
}: {
  agentKey: AgentKey;
  filePath: string;
  lang: string;
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

  // Load file
  useEffect(() => {
    setFileState({ status: "loading" });
    setEditing(false);
    setHighlighted(null);
    setError(null);
    setSaved(false);
    invoke<string | null>("read_agent_config_file", { agent: agentKey })
      .then((c) => {
        if (c === null) {
          setFileState({ status: "missing" });
        } else {
          setFileState({ status: "loaded", content: c });
          setOriginal(c);
        }
      })
      .catch((e) => setError(String(e)));
  }, [agentKey]);

  useEffect(() => {
    void refreshAgentVersion(true);
  }, [refreshAgentVersion]);

  // Re-highlight when content changes
  useEffect(() => {
    if (fileState.status !== "loaded") return;
    let cancelled = false;
    setHighlighted(null);
    highlightCodeToHtml(fileState.content, lang).then((html) => {
      if (!cancelled) {
        setHighlighted(html);
      }
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

  const isDirty = fileState.status === "loaded" && fileState.content !== original;

  return (
    <>
      <div className="ai-settings-body ai-settings-agent-config">
        <div className="ai-settings-version-card">
          <div className="ai-settings-section-head">
            <div className="ai-settings-title-stack">
              <span className="ai-settings-section-title">已安装版本</span>
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

        {/* File path + edit button row */}
        <div className="ai-settings-file-row">
          <div className="ai-settings-path-pill">{filePath}</div>
          {fileState.status === "loaded" && !editing && (
            <button
              className="ai-settings-tool-button"
              onClick={() => setEditing(true)}
              type="button"
            >
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
          <textarea
            autoFocus
            className="ai-settings-textarea"
            style={{ caretColor: "var(--foreground)" }}
            value={fileState.content}
            onChange={(e) => setFileState({ status: "loaded", content: e.target.value })}
            spellCheck={false}
          />
        )}
      </div>

      {editing && (
        <div className="ai-settings-footer">
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
      )}
    </>
  );
}

// ── Main Dialog ───────────────────────────────────────────────────────────────

function SettingsNavIcon({ item, size }: { item: (typeof NAV_ITEMS)[number]; size: number }) {
  if (item.logo) {
    return (
      <img
        src={item.logo}
        alt=""
        className={item.key === "codex" ? "ai-settings-logo is-muted" : "ai-settings-logo"}
        style={{ width: size, height: size }}
      />
    );
  }

  if (item.key === "rag") {
    return <Database size={size} strokeWidth={1.8} />;
  }

  return <Settings2 size={size} strokeWidth={1.8} />;
}

export function AppSettingsDialog({
  onClose,
  initialTab,
  projectId,
  projectPath,
}: {
  onClose: () => void;
  initialTab?: NavKey;
  projectId?: string;
  projectPath?: string;
}) {
  const [activeNav, setActiveNav] = useState<NavKey>(initialTab || "general");

  function handleOverlayClick(e: React.MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }

  const activeItem = NAV_ITEMS.find((n) => n.key === activeNav)!;

  return (
    <div className="ai-dialog-overlay ai-settings-overlay" onClick={handleOverlayClick}>
      <div className="ai-settings-shell ai-migrated-settings">
        {/* Left nav */}
        <nav className="ai-settings-nav" aria-label="应用设置">
          <div className="ai-settings-nav-title">
            <span>应用设置</span>
          </div>
          {NAV_ITEMS.map((item) => (
            <button
              key={item.key}
              type="button"
              className={
                activeNav === item.key ? "ai-settings-nav-item is-active" : "ai-settings-nav-item"
              }
              onClick={() => setActiveNav(item.key)}
            >
              <SettingsNavIcon item={item} size={14} />
              <span className="ai-settings-nav-label">{item.label}</span>
            </button>
          ))}
        </nav>

        {/* Right content */}
        <section className="ai-settings-content">
          <div className="ai-settings-header">
            <div className="ai-settings-title-wrap">
              <SettingsNavIcon item={activeItem} size={16} />
              <span className="ai-settings-content-title">{activeItem.label}</span>
            </div>
            <button className="ai-settings-close" onClick={onClose} title="关闭" type="button">
              <X size={16} strokeWidth={2} />
            </button>
          </div>

          <div className="ai-settings-panel-host">
            {activeNav === "general" ? (
              <GeneralPanel key="general" />
            ) : activeNav === "aha" ? (
              <AhaAgentPanel key="aha" projectPath={projectPath} />
            ) : activeNav === "rag" ? (
              <RagKbConfigPanel key="rag" projectId={projectId} projectPath={projectPath} />
            ) : (
              <AgentConfigPanel
                key={activeNav}
                agentKey={activeNav as AgentKey}
                filePath={activeItem.filePath!}
                lang={activeItem.lang!}
              />
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
