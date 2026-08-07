import { useEffect, useState } from "react";
import { isDarkActive, THEME_CHANGE_EVENT } from "../lib/theme";

export function useIsDarkTheme(): boolean {
  const [dark, setDark] = useState(isDarkActive);

  useEffect(() => {
    const handleChange = () => setDark(isDarkActive());
    document.addEventListener(THEME_CHANGE_EVENT, handleChange);
    return () => document.removeEventListener(THEME_CHANGE_EVENT, handleChange);
  }, []);

  return dark;
}
