export type ThemePreference = "system" | "light" | "dark";

const THEME_STORAGE_KEY = "jkcodingagent.theme";
const DARK_MODE_QUERY = "(prefers-color-scheme: dark)";
export const THEME_CHANGE_EVENT = "aha:theme";

function isThemePreference(value: string | null): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function getStoredThemePreference(): ThemePreference {
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return isThemePreference(stored) ? stored : "system";
}

/** 后端 settings.json 中的 theme 是自由字符串，收敛为合法值，非法时回退 system。 */
export function normalizeThemePreference(value: unknown): ThemePreference {
  return value === "light" || value === "dark" || value === "system" ? value : "system";
}

export function applyThemePreference(preference: ThemePreference): void {
  const dark =
    preference === "dark" ||
    (preference === "system" && window.matchMedia(DARK_MODE_QUERY).matches);
  document.documentElement.classList.toggle("dark", dark);
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
  document.dispatchEvent(new CustomEvent(THEME_CHANGE_EVENT, { detail: { dark } }));
}

export function isDarkActive(): boolean {
  return document.documentElement.classList.contains("dark");
}

export function persistThemePreference(preference: ThemePreference): void {
  window.localStorage.setItem(THEME_STORAGE_KEY, preference);
  applyThemePreference(preference);
}

export function initializeTheme(): void {
  applyThemePreference(getStoredThemePreference());
  window.matchMedia(DARK_MODE_QUERY).addEventListener("change", () => {
    const preference = getStoredThemePreference();
    if (preference === "system") applyThemePreference(preference);
  });
}
