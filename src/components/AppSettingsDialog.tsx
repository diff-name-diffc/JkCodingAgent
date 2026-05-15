import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Pencil, Check, RefreshCw, Monitor } from "lucide-react";
import type { ThemeMode } from "../types";
import s from "../styles";
import claudeLogo from "../assets/claude.svg";
import chatgptLogo from "../assets/chatgpt.svg";
import appLogo from "../assets/app-logo.png";

// Reuse the same singleton highlighter as FileViewer
import type { Highlighter } from "shiki";
let _highlighterPromise: Promise<Highlighter> | null = null;
function getHighlighter(): Promise<Highlighter> {
  if (!_highlighterPromise) {
    _highlighterPromise = import("shiki").then(({ createHighlighter }) =>
      createHighlighter({ themes: ["github-dark", "github-light"], langs: ["json", "toml"] }),
    );
  }
  return _highlighterPromise!;
}

type NavKey = "general" | "theme" | "aha" | "claude" | "codex";

interface AppSettings {
  claude_path: string;
  codex_path: string;
}

interface AgentVersions {
  claude_version: string;
  codex_version: string;
}

type AgentKey = "claude" | "codex";

const DEFAULT_SUMMARY_MODEL = "deepseek-v4-flash";

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

function AhaAgentPanel() {
  const [apiBase, setApiBase] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [summaryModel, setSummaryModel] = useState(DEFAULT_SUMMARY_MODEL);
  const [visionModel, setVisionModel] = useState("");
  const [asrApiKey, setAsrApiKey] = useState("");
  const [asrWebsocketUrl, setAsrWebsocketUrl] = useState("");
  const [imageModelUrl, setImageModelUrl] = useState("");
  const [imageModelApiKey, setImageModelApiKey] = useState("");
  const [imageModel, setImageModel] = useState("");
  const [autoApprove, setAutoApprove] = useState(false);
  const [contextDebug, setContextDebug] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [modelList, setModelList] = useState<string[]>([]);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);

  const [showKey, setShowKey] = useState(false);
  const [showModelDropdown, setShowModelDropdown] = useState(false);

  useEffect(() => {
    invoke<{
      apiBase: string;
      apiKey: string;
      model: string;
      summaryModel: string;
      visionModel: string;
      asrApiKey: string;
      asrWebsocketUrl: string;
      autoApproveDispatch: boolean;
      contextDebug: boolean;
      imageModelUrl: string;
      imageModelApiKey: string;
      imageModel: string;
    } | null>("dispatcher_get_settings")
      .then((settings) => {
        if (!settings) return;
        setApiBase(settings.apiBase);
        setApiKey(settings.apiKey);
        setModel(settings.model);
        setSummaryModel(settings.summaryModel?.trim() || DEFAULT_SUMMARY_MODEL);
        setVisionModel(settings.visionModel ?? "");
        setAsrApiKey(settings.asrApiKey ?? "");
        setAsrWebsocketUrl(settings.asrWebsocketUrl ?? "");
        setImageModelUrl(settings.imageModelUrl ?? "https://dashscope.aliyuncs.com/api/v1");
        setImageModelApiKey(settings.imageModelApiKey ?? "");
        setImageModel(settings.imageModel ?? "qwen-image-2.0-pro");
        setAutoApprove(settings.autoApproveDispatch);
        setContextDebug(settings.contextDebug);
      })
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  async function handleSave() {
    setSaving(true);
    setSaved(false);
    setSaveError(null);
    try {
      const savedSettings = await invoke<{
        apiBase: string;
        apiKey: string;
        model: string;
        summaryModel: string;
        visionModel: string;
        asrApiKey: string;
        asrWebsocketUrl: string;
        autoApproveDispatch: boolean;
        contextDebug: boolean;
        imageModelUrl: string;
        imageModelApiKey: string;
        imageModel: string;
      }>("dispatcher_save_settings", {
        apiBase,
        apiKey,
        model,
        summaryModel,
        visionModel,
        asrApiKey,
        asrWebsocketUrl,
        autoApproveDispatch: autoApprove,
        contextDebug,
        imageModelUrl,
        imageModelApiKey,
        imageModel,
      });
      if (savedSettings.contextDebug !== contextDebug) {
        setContextDebug(savedSettings.contextDebug);
        setSaveError(
          "上下文调试开关尚未被后端接受。若刚修改了 `src-tauri` 代码，请先重启 `pnpm tauri dev` 后再保存一次。",
        );
        return;
      }
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error(e);
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function handleFetchModels() {
    setFetchingModels(true);
    setFetchError(null);
    try {
      const models = await invoke<string[]>("dispatcher_fetch_models", { apiBase, apiKey });
      setModelList(models);
      setShowModelDropdown(true);
    } catch (err) {
      setFetchError(String(err));
      setModelList([]);
    } finally {
      setFetchingModels(false);
    }
  }

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

  const hintStyle: React.CSSProperties = {
    fontSize: 11,
    color: "var(--text-hint)",
    marginTop: 3,
  };

  const filteredModels = model
    ? modelList.filter((m) => m.toLowerCase().includes(model.toLowerCase()))
    : modelList;

  return (
    <>
      <div style={{ ...s.settingsBody, padding: "20px" }}>
        {loading ? (
          <div style={{ color: "var(--text-hint)", fontSize: 13 }}>加载中...</div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
            <div>
              <label style={labelStyle}>API 基础地址</label>
              <input
                style={inputStyle}
                value={apiBase}
                onChange={(e) => setApiBase(e.target.value)}
                placeholder="https://api.openai.com/v1"
                spellCheck={false}
              />
              <span style={hintStyle}>OpenAI 兼容 API 地址，如 DashScope、DeepSeek 等</span>
            </div>

            <div>
              <label style={labelStyle}>API Key</label>
              <div style={{ position: "relative" }}>
                <input
                  style={{ ...inputStyle, paddingRight: 36 }}
                  type={showKey ? "text" : "password"}
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="sk-..."
                  spellCheck={false}
                />
                <button
                  style={{
                    position: "absolute",
                    right: 8,
                    top: "50%",
                    transform: "translateY(-50%)",
                    background: "none",
                    border: "none",
                    color: "var(--text-secondary)",
                    cursor: "pointer",
                    fontSize: 14,
                  }}
                  onClick={() => setShowKey(!showKey)}
                >
                  {showKey ? "🙈" : "👁"}
                </button>
              </div>
            </div>

            <div>
              <label style={labelStyle}>ASR API Key</label>
              <input
                style={inputStyle}
                type={showKey ? "text" : "password"}
                value={asrApiKey}
                onChange={(e) => setAsrApiKey(e.target.value)}
                placeholder="留空则回退 DASHSCOPE_API_KEY"
                spellCheck={false}
              />
              <span style={hintStyle}>实时语音识别专用 DashScope API Key。</span>
            </div>

            <div>
              <label style={labelStyle}>ASR WebSocket 地址</label>
              <input
                style={inputStyle}
                value={asrWebsocketUrl}
                onChange={(e) => setAsrWebsocketUrl(e.target.value)}
                placeholder="wss://dashscope.aliyuncs.com/api-ws/v1/inference"
                spellCheck={false}
              />
              <span style={hintStyle}>可选。留空时根据 API 基础地址自动选择 DashScope 国内/国际实时识别地址。</span>
            </div>

            <div style={{ position: "relative" }}>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "baseline",
                  marginBottom: 5,
                }}
              >
                <label style={{ ...labelStyle, marginBottom: 0 }}>模型</label>
                <button
                  onClick={handleFetchModels}
                  disabled={fetchingModels || !apiBase || !apiKey}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 4,
                    background: "transparent",
                    border: "none",
                    color: "var(--accent)",
                    fontSize: 11.5,
                    cursor: fetchingModels || !apiBase || !apiKey ? "default" : "pointer",
                    opacity: fetchingModels || !apiBase || !apiKey ? 0.6 : 1,
                  }}
                >
                  <RefreshCw size={11} className={fetchingModels ? "spin" : undefined} />
                  {fetchingModels ? "获取中..." : "获取模型"}
                </button>
              </div>

              <div style={{ position: "relative" }}>
                <input
                  style={{
                    ...inputStyle,
                    paddingRight: modelList.length > 0 ? 30 : 10,
                  }}
                  value={model}
                  onChange={(e) => {
                    setModel(e.target.value);
                    if (modelList.length > 0) setShowModelDropdown(true);
                  }}
                  onFocus={() => {
                    if (modelList.length > 0) setShowModelDropdown(true);
                  }}
                  placeholder="例如 gpt-4o、qwen-plus"
                  spellCheck={false}
                />
                {modelList.length > 0 && (
                  <button
                    onClick={() => setShowModelDropdown(!showModelDropdown)}
                    style={{
                      position: "absolute",
                      right: 1,
                      top: 1,
                      bottom: 1,
                      width: 28,
                      background: "transparent",
                      border: "none",
                      color: "var(--text-secondary)",
                      cursor: "pointer",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                    }}
                  >
                    <div
                      style={{
                        transform: showModelDropdown ? "rotate(180deg)" : "none",
                        transition: "transform 0.2s",
                        fontSize: 10,
                      }}
                    >
                      ▼
                    </div>
                  </button>
                )}

                {showModelDropdown && modelList.length > 0 && (
                  <div
                    style={{
                      position: "absolute",
                      top: "100%",
                      left: 0,
                      right: 0,
                      marginTop: 4,
                      background: "var(--bg-card)",
                      border: "1px solid var(--border-medium)",
                      borderRadius: 8,
                      boxShadow: "0 4px 12px rgba(0,0,0,0.2)",
                      maxHeight: 180,
                      overflowY: "auto",
                      zIndex: 100,
                      padding: 4,
                    }}
                  >
                    {filteredModels.length === 0 ? (
                      <div
                        style={{
                          padding: "8px 12px",
                          fontSize: 12,
                          color: "var(--text-hint)",
                          textAlign: "center",
                        }}
                      >
                        没有匹配的模型
                      </div>
                    ) : (
                      filteredModels.map((m) => (
                        <div
                          key={m}
                          onClick={() => {
                            setModel(m);
                            setShowModelDropdown(false);
                          }}
                          style={{
                            padding: "8px 12px",
                            fontSize: 12,
                            fontFamily: "var(--font-mono)",
                            color: m === model ? "var(--accent)" : "var(--text-primary)",
                            background: m === model ? "var(--accent-subtle)" : "transparent",
                            cursor: "pointer",
                            borderRadius: 4,
                          }}
                          onMouseEnter={(e) => {
                            if (m !== model) e.currentTarget.style.background = "var(--bg-hover)";
                          }}
                          onMouseLeave={(e) => {
                            if (m !== model) e.currentTarget.style.background = "transparent";
                          }}
                        >
                          {m}
                        </div>
                      ))
                    )}
                  </div>
                )}
              </div>
              {fetchError && (
                <div style={{ fontSize: 11, color: "var(--danger)", marginTop: 6 }}>
                  获取失败：{fetchError}
                </div>
              )}
            </div>

            <div>
              <label style={labelStyle}>摘要模型</label>
              <input
                style={inputStyle}
                value={summaryModel}
                onChange={(e) => setSummaryModel(e.target.value)}
                placeholder={DEFAULT_SUMMARY_MODEL}
                spellCheck={false}
              />
              <span style={hintStyle}>用于工具结果和子任务输出摘要，留空时默认使用 {DEFAULT_SUMMARY_MODEL}。</span>
            </div>

            <div>
              <label style={labelStyle}>视觉模型</label>
              <input
                style={inputStyle}
                value={visionModel}
                onChange={(e) => setVisionModel(e.target.value)}
                placeholder="例如 qwen-vl-plus、gpt-4o"
                spellCheck={false}
              />
              <span style={hintStyle}>
                用户上传图片时自动切换到该模型；留空时图片请求会停止并提示配置缺失。
              </span>
            </div>

            <div>
              <label style={labelStyle}>图像模型地址</label>
              <input
                style={inputStyle}
                value={imageModelUrl}
                onChange={(e) => setImageModelUrl(e.target.value)}
                placeholder="https://dashscope.aliyuncs.com/api/v1"
                spellCheck={false}
              />
              <span style={hintStyle}>图片生成 API 基础地址，默认使用 DashScope</span>
            </div>

            <div>
              <label style={labelStyle}>图像模型 API Key</label>
              <input
                style={inputStyle}
                type={showKey ? "text" : "password"}
                value={imageModelApiKey}
                onChange={(e) => setImageModelApiKey(e.target.value)}
                placeholder="留空则回退 DASHSCOPE_API_KEY"
                spellCheck={false}
              />
              <span style={hintStyle}>图片生成专用 API Key，留空时回退到主 API Key</span>
            </div>

            <div>
              <label style={labelStyle}>图像模型名称</label>
              <input
                style={inputStyle}
                value={imageModel}
                onChange={(e) => setImageModel(e.target.value)}
                placeholder="qwen-image-2.0-pro"
                spellCheck={false}
              />
              <span style={hintStyle}>图片生成模型名称，如 qwen-image-2.0-pro</span>
            </div>

            <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 12 }}>
              <input
                type="checkbox"
                checked={autoApprove}
                onChange={(e) => setAutoApprove(e.target.checked)}
                style={{ accentColor: "var(--accent)", cursor: "pointer", width: 14, height: 14 }}
              />
              <span
                style={{
                  fontSize: 12.5,
                  color: "var(--text-primary)",
                  userSelect: "none",
                  cursor: "pointer",
                }}
                onClick={() => setAutoApprove(!autoApprove)}
              >
                自动批准操作
              </span>
            </div>
            <span
              style={{
                fontSize: 11,
                color: "var(--text-hint)",
                marginLeft: 22,
                marginTop: -6,
                display: "block",
              }}
            >
              开启后，Aha 智能体在运行子任务前不再额外请求确认。
            </span>

            <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 4 }}>
              <input
                type="checkbox"
                checked={contextDebug}
                onChange={(e) => setContextDebug(e.target.checked)}
                style={{ accentColor: "var(--accent)", cursor: "pointer", width: 14, height: 14 }}
              />
              <span
                style={{
                  fontSize: 12.5,
                  color: "var(--text-primary)",
                  userSelect: "none",
                  cursor: "pointer",
                }}
                onClick={() => setContextDebug(!contextDebug)}
              >
                上下文调试日志
              </span>
            </div>
            <span
              style={{
                fontSize: 11,
                color: "var(--text-hint)",
                marginLeft: 22,
                marginTop: -6,
                display: "block",
              }}
            >
              仅在调试时开启。只记录实际发送给大模型的请求/响应快照，其中包含摘要后的注入内容；
              不会写入子任务终端原始输出。日志文件位于项目根目录的{" "}
              <code>logs/agent.debug</code>。
            </span>
          </div>
        )}
      </div>
      <div style={s.settingsFooter}>
        {saveError && (
          <span
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: 12,
              color: "var(--danger)",
              marginRight: "auto",
              maxWidth: "70%",
            }}
          >
            {saveError}
          </span>
        )}
        {saved && (
          <span
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: 12,
              color: "var(--success, #34c759)",
              marginRight: saveError ? 12 : "auto",
            }}
          >
            <Check size={12} /> 已保存
          </span>
        )}
        <button
          style={{ ...s.modalSaveBtn, opacity: saving ? 0.5 : 1 }}
          onClick={handleSave}
          disabled={saving}
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
    setHighlighted(null);
    getHighlighter().then((hl) => {
      const html = hl.codeToHtml(fileState.content, {
        lang,
        theme: isDark ? "github-dark" : "github-light",
      });
      setHighlighted(html);
    });
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
}: {
  onClose: () => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  initialTab?: NavKey;
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
            <AhaAgentPanel key="aha" />
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
