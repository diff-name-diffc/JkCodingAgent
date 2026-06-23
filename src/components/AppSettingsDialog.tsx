import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Pencil, Check, RefreshCw, Monitor, Database } from "lucide-react";
import { AhaAgentPanel } from "./app-settings/aha/AhaAgentPanel";
import { RagKbConfigPanel } from "./app-settings/rag/RagKbConfigPanel";
import type { ThemeMode } from "../types";
import s from "../styles";
import claudeLogo from "../assets/claude.svg";
import chatgptLogo from "../assets/chatgpt.svg";
import appLogo from "../assets/app-logo.png";
import { highlightCodeToHtml } from "../utils/shiki";

type NavKey = "general" | "theme" | "aha" | "rag" | "claude" | "codex";

interface AppSettings {
  claude_path: string;
  codex_path: string;
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
  { key: "theme", label: "主题" },
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

interface ThemePanelProps {
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
}

function ThemePanel({ themeMode, systemPrefersDark, onThemeModeChange }: ThemePanelProps) {
  const manualThemeModes: Array<Extract<ThemeMode, "dark" | "light">> = ["dark", "light"];
  const selectedLabel =
    themeMode === "system"
      ? `跟随系统 · ${systemPrefersDark ? "深色" : "浅色"}`
      : `手动设置 · ${themeMode === "dark" ? "深色" : "浅色"}`;

  function handleSystemThemeToggle() {
    onThemeModeChange(themeMode === "system" ? "light" : "system");
  }

  function handleManualThemeKeyDown(
    mode: Extract<ThemeMode, "dark" | "light">,
    event: React.KeyboardEvent<HTMLButtonElement>,
  ) {
    const currentIndex = manualThemeModes.indexOf(mode);
    if (currentIndex === -1) {
      return;
    }

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      event.preventDefault();
      onThemeModeChange(manualThemeModes[(currentIndex + 1) % manualThemeModes.length]);
      return;
    }

    if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      event.preventDefault();
      onThemeModeChange(
        manualThemeModes[(currentIndex - 1 + manualThemeModes.length) % manualThemeModes.length],
      );
      return;
    }

    if (event.key === "Home") {
      event.preventDefault();
      onThemeModeChange(manualThemeModes[0]);
      return;
    }

    if (event.key === "End") {
      event.preventDefault();
      onThemeModeChange(manualThemeModes[manualThemeModes.length - 1]);
    }
  }

  function renderThemeOption({
    mode,
    title,
    description,
    previewBackground,
    previewBorder,
    previewAccent,
  }: {
    mode: Extract<ThemeMode, "dark" | "light">;
    title: string;
    description: string;
    previewBackground: string;
    previewBorder: string;
    previewAccent: string;
  }) {
    const selected = themeMode === mode;

    return (
      <button
        type="button"
        onClick={() => onThemeModeChange(mode)}
        onKeyDown={(event) => handleManualThemeKeyDown(mode, event)}
        role="radio"
        aria-checked={selected}
        aria-label={`${title}主题`}
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "stretch",
          gap: 10,
          padding: 14,
          borderRadius: 12,
          border: `1px solid ${selected ? "var(--accent)" : "var(--border-medium)"}`,
          background: selected ? "var(--accent-subtle)" : "var(--bg-subtle)",
          cursor: "pointer",
          textAlign: "left",
          boxShadow: selected ? "0 0 0 1px var(--accent-subtle)" : "none",
          transition: "border-color 0.12s, background 0.12s, box-shadow 0.12s",
        }}
      >
        <div
          style={{
            width: "100%",
            height: 106,
            borderRadius: 10,
            border: `1px solid ${previewBorder}`,
            background: previewBackground,
            padding: 8,
            boxSizing: "border-box",
            display: "flex",
            flexDirection: "column",
            gap: 7,
            overflow: "hidden",
          }}
        >
          <div style={{ display: "flex", gap: 5 }}>
            <span
              style={{
                width: 7,
                height: 7,
                borderRadius: 999,
                background: previewAccent,
                opacity: 0.9,
              }}
            />
            <span
              style={{
                width: 7,
                height: 7,
                borderRadius: 999,
                background: previewAccent,
                opacity: 0.65,
              }}
            />
            <span
              style={{
                width: 7,
                height: 7,
                borderRadius: 999,
                background: previewAccent,
                opacity: 0.4,
              }}
            />
          </div>
          <div
            style={{
              flex: 1,
              display: "grid",
              gridTemplateColumns: mode === "dark" ? "28px 1fr" : "24px 1fr",
              gap: 7,
            }}
          >
            <div
              style={{
                borderRadius: 7,
                background: mode === "dark" ? "rgba(255,255,255,0.05)" : "rgba(23,27,36,0.06)",
                border:
                  mode === "dark"
                    ? "1px solid rgba(255,255,255,0.06)"
                    : "1px solid rgba(23,27,36,0.06)",
                display: "flex",
                flexDirection: "column",
                gap: 5,
                padding: "7px 5px",
              }}
            >
              <span
                style={{
                  height: 5,
                  borderRadius: 999,
                  background: previewAccent,
                  opacity: mode === "dark" ? 0.55 : 0.3,
                }}
              />
              <span
                style={{
                  height: 5,
                  borderRadius: 999,
                  background: previewAccent,
                  opacity: mode === "dark" ? 0.28 : 0.16,
                }}
              />
              <span
                style={{
                  height: 5,
                  borderRadius: 999,
                  background: previewAccent,
                  opacity: mode === "dark" ? 0.2 : 0.12,
                }}
              />
            </div>
            <div
              style={{
                borderRadius: 8,
                background:
                  mode === "dark"
                    ? "linear-gradient(180deg, rgba(255,255,255,0.08), rgba(255,255,255,0.04))"
                    : "linear-gradient(180deg, rgba(23,27,36,0.1), rgba(23,27,36,0.04))",
                border:
                  mode === "dark"
                    ? "1px solid rgba(255,255,255,0.08)"
                    : "1px solid rgba(23,27,36,0.08)",
                padding: 8,
                boxSizing: "border-box",
                display: "flex",
                flexDirection: "column",
                gap: 6,
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 6,
                }}
              >
                <span
                  style={{
                    width: 34,
                    height: 6,
                    borderRadius: 999,
                    background: previewAccent,
                    opacity: mode === "dark" ? 0.75 : 0.2,
                  }}
                />
                <span
                  style={{
                    width: 12,
                    height: 12,
                    borderRadius: 4,
                    background: mode === "dark" ? "rgba(255,255,255,0.12)" : "#ffffff",
                    border:
                      mode === "dark"
                        ? "1px solid rgba(255,255,255,0.08)"
                        : "1px solid rgba(23,27,36,0.08)",
                  }}
                />
              </div>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "1.15fr 0.85fr",
                  gap: 6,
                  flex: 1,
                }}
              >
                <div
                  style={{
                    borderRadius: 6,
                    background:
                      mode === "dark" ? "rgba(255,255,255,0.07)" : "rgba(255,255,255,0.9)",
                    border:
                      mode === "dark"
                        ? "1px solid rgba(255,255,255,0.06)"
                        : "1px solid rgba(23,27,36,0.06)",
                  }}
                />
                <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
                  <span
                    style={{
                      height: 18,
                      borderRadius: 6,
                      background:
                        mode === "dark" ? "rgba(255,255,255,0.09)" : "rgba(255,255,255,0.92)",
                      border:
                        mode === "dark"
                          ? "1px solid rgba(255,255,255,0.06)"
                          : "1px solid rgba(23,27,36,0.06)",
                    }}
                  />
                  <span
                    style={{
                      flex: 1,
                      borderRadius: 6,
                      background:
                        mode === "dark" ? "rgba(255,255,255,0.05)" : "rgba(255,255,255,0.82)",
                      border:
                        mode === "dark"
                          ? "1px solid rgba(255,255,255,0.05)"
                          : "1px solid rgba(23,27,36,0.05)",
                    }}
                  />
                </div>
              </div>
            </div>
          </div>
        </div>

        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 8,
            }}
          >
            <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)" }}>
              {title}
            </span>
            {selected && <Check size={14} color="var(--accent)" />}
          </div>
          <span style={{ fontSize: 11.5, color: "var(--text-hint)", lineHeight: 1.45 }}>
            {description}
          </span>
        </div>
      </button>
    );
  }

  return (
    <div
      style={{
        ...s.settingsBody,
        display: "flex",
        flexDirection: "column",
        gap: 18,
        padding: "20px",
      }}
    >
      <button
        type="button"
        onClick={handleSystemThemeToggle}
        role="switch"
        aria-checked={themeMode === "system"}
        aria-label="跟随系统主题"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 14,
          padding: "16px 18px",
          borderRadius: 12,
          border: `1px solid ${themeMode === "system" ? "var(--accent)" : "var(--border-dim)"}`,
          background: themeMode === "system" ? "var(--accent-subtle)" : "var(--bg-subtle)",
          cursor: "pointer",
          textAlign: "left",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12, minWidth: 0 }}>
          <div
            style={{
              flexShrink: 0,
              width: 48,
              height: 28,
              borderRadius: 999,
              border: "none",
              padding: 3,
              background: themeMode === "system" ? "var(--accent)" : "var(--border-medium)",
              boxShadow:
                themeMode === "system"
                  ? "0 0 0 4px var(--accent-subtle)"
                  : "inset 0 0 0 1px var(--border-dim)",
              transition: "background 0.12s, box-shadow 0.12s",
            }}
          >
            <div
              style={{
                width: 22,
                height: 22,
                borderRadius: 999,
                display: "grid",
                placeItems: "center",
                background: "#fff",
                color: themeMode === "system" ? "var(--accent)" : "var(--text-secondary)",
                transform: themeMode === "system" ? "translateX(20px)" : "translateX(0)",
                transition: "transform 0.12s ease",
              }}
            >
              <Monitor size={12} />
            </div>
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: 3,
              minWidth: 0,
              padding: 0,
              textAlign: "left",
            }}
          >
            <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)" }}>
              跟随系统
            </span>
          </div>
        </div>
        <div
          style={{
            flexShrink: 0,
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            padding: "6px 10px",
            borderRadius: 999,
            background: "var(--bg-card)",
            border: "1px solid var(--border-medium)",
            color: "var(--text-secondary)",
            fontSize: 11.5,
            fontWeight: 600,
          }}
        >
          {themeMode === "system" && <Check size={13} color="var(--accent)" />}
          {selectedLabel}
        </div>
      </button>

      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)" }}>
          手动主题
        </div>
        <div
          style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))", gap: 14 }}
          role="radiogroup"
          aria-label="手动主题"
        >
          {renderThemeOption({
            mode: "dark",
            title: "深色",
            description: "始终使用深色界面。",
            previewBackground: "#11151d",
            previewBorder: "rgba(255,255,255,0.08)",
            previewAccent: "#f1f4fb",
          })}
          {renderThemeOption({
            mode: "light",
            title: "浅色",
            description: "始终使用浅色界面。",
            previewBackground: "#f5f7fb",
            previewBorder: "rgba(23,27,36,0.08)",
            previewAccent: "#171b24",
          })}
        </div>
      </div>
    </div>
  );
}

// ── General Panel ─────────────────────────────────────────────────────────────

function GeneralPanel() {
  const [settings, setSettings] = useState<AppSettings>({ claude_path: "", codex_path: "" });
  const [original, setOriginal] = useState<AppSettings>({ claude_path: "", codex_path: "" });
  const [loading, setLoading] = useState(true);
  const [detectingPaths, setDetectingPaths] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppSettings>("load_app_settings")
      .then(async (loadedSettings) => {
        setSettings(loadedSettings);
        setOriginal(loadedSettings);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  async function handleDetect() {
    setDetectingPaths(true);
    setError(null);
    try {
      const detected = await invoke<AppSettings>("detect_agent_paths");
      setSettings(detected);
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
    settings.claude_path !== original.claude_path || settings.codex_path !== original.codex_path;

  const inputStyle: React.CSSProperties = {
    width: "100%",
    padding: "7px 10px",
    background: "var(--bg-input)",
    border: "1px solid var(--border-medium)",
    borderRadius: 7,
    color: "var(--text-primary)",
    fontSize: 12.5,
    fontFamily: "var(--font-mono)",
    outline: "none",
    boxSizing: "border-box",
  };

  const labelStyle: React.CSSProperties = {
    fontSize: 12,
    fontWeight: 600,
    color: "var(--text-secondary)",
    marginBottom: 5,
    display: "block",
  };

  const fieldStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: 5,
    marginBottom: 18,
  };

  const hintStyle: React.CSSProperties = {
    fontSize: 11,
    color: "var(--text-hint)",
    marginTop: 3,
  };

  return (
    <>
      <div
        style={{
          ...s.settingsBody,
          display: "flex",
          flexDirection: "column",
          gap: 0,
          padding: "20px 20px 14px",
        }}
      >
        {error && (
          <div style={{ color: "var(--danger)", fontSize: 12.5, marginBottom: 14 }}>{error}</div>
        )}

        {loading ? (
          <div style={{ color: "var(--text-hint)", fontSize: 13 }}>加载中...</div>
        ) : (
          <>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                marginBottom: 18,
              }}
            >
              <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)" }}>
                智能体安装路径
              </span>
              <button
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 5,
                  padding: "5px 10px",
                  background: "none",
                  border: "1px solid var(--border-medium)",
                  borderRadius: 6,
                  fontSize: 12,
                  color: "var(--text-secondary)",
                  cursor: detectingPaths ? "default" : "pointer",
                  opacity: detectingPaths ? 0.6 : 1,
                }}
                onClick={handleDetect}
                disabled={detectingPaths}
              >
                <RefreshCw size={12} className={detectingPaths ? "spin" : undefined} />
                {detectingPaths ? "检测中..." : "自动检测"}
              </button>
            </div>

            <div style={fieldStyle}>
              <label style={labelStyle}>Claude Code 路径</label>
              <input
                style={inputStyle}
                value={settings.claude_path}
                onChange={(e) => setSettings((prev) => ({ ...prev, claude_path: e.target.value }))}
                placeholder="/usr/local/bin/claude"
                spellCheck={false}
              />
              <span style={hintStyle}>留空则使用系统 PATH 中的 `claude`。</span>
            </div>

            <div style={fieldStyle}>
              <label style={labelStyle}>Codex 路径</label>
              <input
                style={inputStyle}
                value={settings.codex_path}
                onChange={(e) => setSettings((prev) => ({ ...prev, codex_path: e.target.value }))}
                placeholder="/usr/local/bin/codex"
                spellCheck={false}
              />
              <span style={hintStyle}>留空则使用系统 PATH 中的 `codex`。</span>
            </div>
          </>
        )}
      </div>

      <div style={s.settingsFooter}>
        {saved && (
          <span
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: 12,
              color: "var(--success, #34c759)",
              marginRight: "auto",
            }}
          >
            <Check size={12} /> 已保存
          </span>
        )}
        <button
          style={{ ...s.modalSaveBtn, opacity: saving || !isDirty ? 0.5 : 1 }}
          onClick={handleSave}
          disabled={saving || !isDirty}
        >
          {saving ? "保存中..." : "保存"}
        </button>
      </div>
    </>
  );
}

// ── Agent Config Panel ────────────────────────────────────────────────────────

type FileState =
  | { status: "loading" }
  | { status: "missing" }
  | { status: "loaded"; content: string };

function AgentConfigPanel({
  agentKey,
  filePath,
  lang,
  isDark,
}: {
  agentKey: AgentKey;
  filePath: string;
  lang: string;
  isDark: boolean;
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

  // Re-highlight when content or theme changes
  useEffect(() => {
    if (fileState.status !== "loaded") return;
    let cancelled = false;
    setHighlighted(null);
    highlightCodeToHtml(fileState.content, lang, isDark).then((html) => {
      if (!cancelled) {
        setHighlighted(html);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [fileState, lang, isDark]);

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
      <div
        style={{
          ...s.settingsBody,
          display: "flex",
          flexDirection: "column",
          gap: 0,
          padding: "14px 20px",
        }}
      >
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 10,
            padding: 14,
            marginBottom: 14,
            borderRadius: 10,
            border: "1px solid var(--border-dim)",
            background: "var(--bg-subtle)",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 12,
            }}
          >
            <div style={{ display: "flex", flexDirection: "column", gap: 3, minWidth: 0 }}>
              <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)" }}>
                已安装版本
              </span>
              <span style={{ fontSize: 11.5, color: "var(--text-hint)", lineHeight: 1.45 }}>
                {versionSourcePath
                  ? `当前可执行文件：${versionSourcePath}`
                  : "当前使用系统 PATH 中的可执行文件。"}
              </span>
            </div>
            <button
              style={{
                display: "flex",
                alignItems: "center",
                gap: 5,
                padding: "5px 10px",
                background: "none",
                border: "1px solid var(--border-medium)",
                borderRadius: 6,
                fontSize: 12,
                color: "var(--text-secondary)",
                cursor: detectingVersion ? "default" : "pointer",
                opacity: detectingVersion ? 0.6 : 1,
                flexShrink: 0,
              }}
              onClick={() => void refreshAgentVersion(false)}
              disabled={detectingVersion}
            >
              <RefreshCw size={12} className={detectingVersion ? "spin" : undefined} />
              {detectingVersion ? "检测中..." : "检测"}
            </button>
          </div>

          <input
            style={{
              width: "100%",
              padding: "7px 10px",
              background: "var(--bg-input)",
              border: "1px solid var(--border-medium)",
              borderRadius: 7,
              color: "var(--text-primary)",
              fontSize: 12.5,
              fontFamily: "var(--font-mono)",
              outline: "none",
              boxSizing: "border-box",
            }}
            value={detectedVersion}
            readOnly
            placeholder={versionLoading ? "检测中..." : "未检测到"}
            spellCheck={false}
          />

          {versionError && (
            <div style={{ fontSize: 11.5, color: "var(--danger)" }}>{versionError}</div>
          )}
        </div>

        {/* File path + edit button row */}
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12 }}>
          <div
            style={{
              fontSize: 11.5,
              color: "var(--text-hint)",
              fontFamily: "var(--font-mono)",
              background: "var(--bg-subtle)",
              border: "1px solid var(--border-dim)",
              borderRadius: 6,
              padding: "4px 9px",
            }}
          >
            {filePath}
          </div>
          {fileState.status === "loaded" && !editing && (
            <button
              style={{
                display: "flex",
                alignItems: "center",
                gap: 5,
                padding: "4px 10px",
                background: "none",
                border: "1px solid var(--border-medium)",
                borderRadius: 6,
                fontSize: 12,
                color: "var(--text-secondary)",
                cursor: "pointer",
              }}
              onClick={() => setEditing(true)}
            >
              <Pencil size={12} />
              编辑
            </button>
          )}
          {saved && (
            <span
              style={{
                display: "flex",
                alignItems: "center",
                gap: 4,
                fontSize: 12,
                color: "var(--success, #34c759)",
              }}
            >
              <Check size={12} /> 已保存
            </span>
          )}
        </div>

        {error && (
          <div style={{ color: "var(--danger)", fontSize: 12.5, marginBottom: 10 }}>{error}</div>
        )}

        {fileState.status === "loading" && !error && (
          <div style={{ color: "var(--text-hint)", fontSize: 13 }}>加载中...</div>
        )}

        {fileState.status === "missing" && (
          <div style={{ color: "var(--text-muted)", fontSize: 13 }}>未找到配置文件</div>
        )}

        {fileState.status === "loaded" && !editing && (
          <div
            className="file-viewer-code"
            style={{
              flex: 1,
              overflowY: "auto",
              borderRadius: 8,
              border: "1px solid var(--border-dim)",
              fontSize: 12.5,
            }}
            dangerouslySetInnerHTML={{ __html: highlighted ?? "" }}
          />
        )}

        {fileState.status === "loaded" && editing && (
          <textarea
            autoFocus
            style={{
              ...s.modalTextarea,
              flex: 1,
              width: "100%",
              minHeight: 300,
              resize: "none",
              boxSizing: "border-box",
              caretColor: isDark ? "#F1F4FB" : "#171B24",
            }}
            value={fileState.content}
            onChange={(e) => setFileState({ status: "loaded", content: e.target.value })}
            spellCheck={false}
          />
        )}
      </div>

      {editing && (
        <div style={s.settingsFooter}>
          <button style={s.modalCancelBtn} onClick={handleCancel}>
            取消
          </button>
          <button
            style={{ ...s.modalSaveBtn, opacity: saving || !isDirty ? 0.5 : 1 }}
            onClick={handleSave}
            disabled={saving || !isDirty}
          >
            {saving ? "保存中..." : "保存"}
          </button>
        </div>
      )}
    </>
  );
}

// ── Main Dialog ───────────────────────────────────────────────────────────────

export function AppSettingsDialog({
  onClose,
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  initialTab,
  projectPath,
}: {
  onClose: () => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  initialTab?: NavKey;
  projectPath?: string;
}) {
  const [activeNav, setActiveNav] = useState<NavKey>(initialTab || "general");

  function handleOverlayClick(e: React.MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }

  const activeItem = NAV_ITEMS.find((n) => n.key === activeNav)!;

  return (
    <div style={s.modalOverlay} onClick={handleOverlayClick}>
      <div style={s.modalBox}>
        {/* Left nav */}
        <div style={s.settingsNav}>
          <div style={s.settingsNavTitle}>应用设置</div>
          {NAV_ITEMS.map((item) => (
            <button
              key={item.key}
              style={{
                ...s.settingsNavItem,
                background: activeNav === item.key ? "var(--bg-hover)" : "none",
                color: activeNav === item.key ? "var(--text-primary)" : "var(--text-secondary)",
                fontWeight: activeNav === item.key ? 600 : 500,
              }}
              onClick={() => setActiveNav(item.key)}
            >
              {item.logo ? (
                <img
                  src={item.logo}
                  style={{ width: 14, height: 14, opacity: item.key === "codex" ? 0.7 : 1 }}
                />
              ) : item.key === "theme" ? (
                <Monitor size={14} strokeWidth={1.8} />
              ) : (
                <span
                  style={{
                    width: 14,
                    height: 14,
                    display: "inline-flex",
                    alignItems: "center",
                    justifyContent: "center",
                    fontSize: 13,
                  }}
                >
                  ⚙
                </span>
              )}
              {item.label}
            </button>
          ))}
        </div>

        {/* Right content */}
        <div style={s.settingsContent}>
          <div style={s.settingsContentHeader}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              {activeItem.logo ? (
                <img
                  src={activeItem.logo}
                  style={{ width: 16, height: 16, opacity: activeItem.key === "codex" ? 0.7 : 1 }}
                />
              ) : activeItem.key === "theme" ? (
                <Monitor size={16} strokeWidth={1.8} color="var(--text-secondary)" />
              ) : activeItem.key === "rag" ? (
                <Database size={16} strokeWidth={1.8} color="var(--text-secondary)" />
              ) : (
                <span style={{ fontSize: 15 }}>⚙</span>
              )}
              <span style={s.settingsContentTitle}>{activeItem.label}</span>
            </div>
            <button style={s.modalCloseBtn} onClick={onClose} title="关闭">
              <X size={16} strokeWidth={2} />
            </button>
          </div>

          {activeNav === "general" ? (
            <GeneralPanel key="general" />
          ) : activeNav === "theme" ? (
            <ThemePanel
              key="theme"
              themeMode={themeMode}
              systemPrefersDark={systemPrefersDark}
              onThemeModeChange={onThemeModeChange}
            />
          ) : activeNav === "aha" ? (
            <AhaAgentPanel key="aha" projectPath={projectPath} />
          ) : activeNav === "rag" ? (
            <RagKbConfigPanel key="rag" projectPath={projectPath} />
          ) : (
            <AgentConfigPanel
              key={activeNav}
              agentKey={activeNav as AgentKey}
              filePath={activeItem.filePath!}
              lang={activeItem.lang!}
              isDark={isDark}
            />
          )}
        </div>
      </div>
    </div>
  );
}
