import { Monitor, Moon, Sun } from "lucide-react";
import {
  normalizeThemePreference,
  persistThemePreference,
  type ThemePreference,
} from "../../lib/theme";
import { useAhaSettings } from "./use-aha-settings";

const THEME_OPTIONS: Array<{
  value: ThemePreference;
  label: string;
  icon: typeof Monitor;
}> = [
  { value: "system", label: "跟随系统", icon: Monitor },
  { value: "light", label: "浅色", icon: Sun },
  { value: "dark", label: "深色", icon: Moon },
];

/**
 * 「通用」页：外观主题。即点即生效——先落 DOM/本地缓存（立即预览），
 * 再经 use-aha-settings 自动保存管线写入 `AhaSettingsV2.theme`。
 */
export function GeneralPage() {
  const { settings, updateSettings } = useAhaSettings();
  if (!settings) return null;
  const current = normalizeThemePreference(settings.theme);

  return (
    <div className="ai-set-page">
      <div className="ai-set-field">
        <span className="ai-set-field-label">外观</span>
        <div className="ai-theme-options" role="radiogroup" aria-label="外观主题">
          {THEME_OPTIONS.map(({ value, label, icon: Icon }) => (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={current === value}
              className={current === value ? "ai-theme-option is-active" : "ai-theme-option"}
              onClick={() => {
                persistThemePreference(value);
                updateSettings((prev) => ({ ...prev, theme: value }), "theme");
              }}
            >
              <Icon size={14} />
              {label}
            </button>
          ))}
        </div>
        <span className="ai-settings-hint">默认跟随操作系统，可随时手动覆盖。</span>
      </div>
    </div>
  );
}
