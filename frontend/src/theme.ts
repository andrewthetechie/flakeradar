// Dark by default; the choice lives on <html data-theme> (set before first
// paint by index.html) and is remembered per browser.
import { useCallback, useState } from "react";

export type Theme = "dark" | "light";

const STORAGE_KEY = "flakeradar-theme";

function currentTheme(): Theme {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

export function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(currentTheme);
  const toggle = useCallback(() => {
    const next: Theme = currentTheme() === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Storage blocked (private mode): the theme still applies for this visit.
    }
    setTheme(next);
  }, []);
  return [theme, toggle];
}
