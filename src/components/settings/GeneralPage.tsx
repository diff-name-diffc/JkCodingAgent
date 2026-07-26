import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Monitor, Moon, RefreshCw, Sun } from "lucide-react";
import {
  applyThemePreference,
  normalizeThemePreference,
  persistThemePreference,
  type ThemePreference,
} from "../../lib/theme";

interface AppSettings {
  claude_path: string;
  codex_path: string;
  theme: ThemePreference;
}

/**
 * 「通用」页：外观主题 + 智能体安装路径。
 * 保留手动保存（主题需要立即生效与回滚），通过 reportDirty 向弹窗外壳上报未保存状态。
 */
export function GeneralPage({ reportDirty }: { reportDirty: (dirty: boolean) => void }) {
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
        loadedSettings.theme = normalizeThemePreference(loadedSettings.theme);
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

  const isDirty =
    settings.claude_path !== original.claude_path ||
    settings.codex_path !== original.codex_path ||
    settings.theme !== original.theme;

  useEffect(() => {
    reportDirty(isDirty);
  }, [isDirty, reportDirty]);

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

  if (loading) {
    return <div className="ai-settings-empty">加载中...</div>;
  }

  return (
    <div className="ai-set-page">
      {error && <div className="ai-settings-error">{error}</div>}

      <div className="ai-set-field">
        <span className="ai-set-field-label">外观</span>
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

      <div className="ai-set-field">
        <label className="ai-set-field-label">Claude Code 路径</label>
        <input
          className="ai-settings-input"
          value={settings.claude_path}
          onChange={(e) => setSettings((prev) => ({ ...prev, claude_path: e.target.value }))}
          placeholder="/usr/local/bin/claude"
          spellCheck={false}
        />
        <span className="ai-settings-hint">留空则使用系统 PATH 中的 `claude`。</span>
      </div>

      <div className="ai-set-field">
        <label className="ai-set-field-label">Codex 路径</label>
        <input
          className="ai-settings-input"
          value={settings.codex_path}
          onChange={(e) => setSettings((prev) => ({ ...prev, codex_path: e.target.value }))}
          placeholder="/usr/local/bin/codex"
          spellCheck={false}
        />
        <span className="ai-settings-hint">留空则使用系统 PATH 中的 `codex`。</span>
      </div>

      <div className="ai-set-page-footer">
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
    </div>
  );
}
