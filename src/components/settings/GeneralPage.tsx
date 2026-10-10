import { useId } from "react";
import { Check, Monitor, Moon, Sun } from "lucide-react";
import {
  normalizeThemePreference,
  persistThemePreference,
  type ThemePreference,
} from "../../lib/theme";
import { useAhaSettings } from "./use-aha-settings";

const THEME_OPTIONS: Array<{
  value: ThemePreference;
  label: string;
  description: string;
  icon: typeof Monitor;
}> = [
  { value: "light", label: "晴昼", description: "暖白底色，清晰舒适", icon: Sun },
  { value: "dark", label: "静夜", description: "柔和暗色，专注当下", icon: Moon },
  { value: "system", label: "跟随系统", description: "随设备外观自动切换", icon: Monitor },
];

function ThemePreview({ theme }: { theme: ThemePreference }) {
  return (
    <span className="ai-theme-preview" data-theme={theme} aria-hidden="true">
      <span className="ai-theme-preview-sidebar">
        <span />
        <span />
        <span />
      </span>
      <span className="ai-theme-preview-main">
        <span className="ai-theme-preview-title" />
        <span className="ai-theme-preview-line" />
        <span className="ai-theme-preview-line" />
        <span className="ai-theme-preview-composer">
          <span />
        </span>
      </span>
    </span>
  );
}

/**
 * 「通用」页：外观主题。即点即生效——先落 DOM/本地缓存（立即预览），
 * 再经 use-aha-settings 自动保存管线写入 `AhaSettingsV2.theme`。
 */
export function GeneralPage() {
  const themeGroupId = useId();
  const { settings, updateSettings, saveError } = useAhaSettings();
  if (!settings) return null;
  const current = normalizeThemePreference(settings.theme);

  return (
    <div className="ai-set-page">
      <fieldset className="ai-theme-fieldset" aria-describedby={`${themeGroupId}-hint`}>
        <legend className="ai-set-field-label">工作区主题</legend>
        <p className="ai-theme-intro">让工作区适合此刻的光线，也适合你的节奏。</p>
        <div className="ai-theme-options">
          {THEME_OPTIONS.map(({ value, label, description, icon: Icon }) => (
            <label key={value} className="ai-theme-choice">
              <input
                className="sr-only"
                type="radio"
                name={themeGroupId}
                value={value}
                checked={current === value}
                onChange={() => {
                  persistThemePreference(value);
                  updateSettings((prev) => ({ ...prev, theme: value }), "theme");
                }}
              />
              <span className="ai-theme-card">
                <ThemePreview theme={value} />
                <span className="ai-theme-card-heading">
                  <Icon size={15} aria-hidden="true" />
                  <span>{label}</span>
                  <span className="ai-theme-check" aria-hidden="true">
                    <Check size={11} strokeWidth={2.5} />
                  </span>
                </span>
                <span className="ai-theme-card-description">{description}</span>
              </span>
            </label>
          ))}
        </div>
        <p className="ai-settings-hint" id={`${themeGroupId}-hint`}>
          选择后即刻生效，偏好会自动保存。
        </p>
        {saveError?.fieldId === "theme" && (
          <span className="ai-set-field-error">{saveError.message}</span>
        )}
      </fieldset>
    </div>
  );
}
