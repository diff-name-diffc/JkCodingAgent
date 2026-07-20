export type ThemePreference = "system" | "light" | "dark";

const THEME_STORAGE_KEY = "jkcodingagent.theme";
const DARK_MODE_QUERY = "(prefers-color-scheme: dark)";

function isThemePreference(value: string | null): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function getStoredThemePreference(): ThemePreference {
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return isThemePreference(stored) ? stored : "system";
}

export function applyThemePreference(preference: ThemePreference): void {
  const dark =
    preference === "dark" ||
    (preference === "system" && window.matchMedia(DARK_MODE_QUERY).matches);
  document.documentElement.classList.toggle("dark", dark);
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
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
