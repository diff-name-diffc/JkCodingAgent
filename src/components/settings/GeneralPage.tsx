import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Monitor, Moon, Sun } from "lucide-react";
import {
  applyThemePreference,
  normalizeThemePreference,
  persistThemePreference,
  type ThemePreference,
} from "../../lib/theme";

interface AppSettings {
  theme: ThemePreference;
}

/** 「通用」页：主题立即预览，保存前关闭则回滚。 */
export function GeneralPage({ reportDirty }: { reportDirty: (dirty: boolean) => void }) {
  const emptySettings: AppSettings = { theme: "system" };
  const [settings, setSettings] = useState<AppSettings>(emptySettings);
  const [original, setOriginal] = useState<AppSettings>(emptySettings);
  const [loading, setLoading] = useState(true);
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

  const isDirty = settings.theme !== original.theme;

  useEffect(() => {
    reportDirty(isDirty);
  }, [isDirty, reportDirty]);

  async function handleSave() {
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      // 主题权威源是后端 app_config，先落库成功后再写本地缓存/DOM（persistThemePreference），
      // 避免 invoke 失败时 DB 与 localStorage 不一致。
      await invoke("save_app_settings", { settings });
      persistThemePreference(settings.theme);
      savedThemeRef.current = settings.theme;
      setOriginal(settings);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      // 保存失败：本地缓存未写入，回滚点击时的即时预览——DOM 与选中状态
      // 都回到已保存主题，避免「高亮未保存项却显示旧主题」的分叉。
      applyThemePreference(original.theme);
      setSettings(original);
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
