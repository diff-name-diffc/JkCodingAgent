import { useEffect, useState } from "react";

function readIsDarkTheme() {
  return document.documentElement.classList.contains("dark");
}

export function useIsDarkTheme() {
  const [isDark, setIsDark] = useState(readIsDarkTheme);

  useEffect(() => {
    const root = document.documentElement;
    const observer = new MutationObserver(() => {
      setIsDark(readIsDarkTheme());
    });

    observer.observe(root, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => observer.disconnect();
  }, []);

  return isDark;
}
