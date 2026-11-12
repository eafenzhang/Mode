type WebThemeSeed = "light" | "dark" | "zai-light" | "zai-dark" | "system";

export const WEB_DEFAULT_THEME: WebThemeSeed = "zai-dark";

function isWebThemeSeed(value: unknown): value is WebThemeSeed {
  return (
    value === "light" ||
    value === "dark" ||
    value === "zai-light" ||
    value === "zai-dark" ||
    value === "system"
  );
}

function normalizeWebThemeSeed(theme: WebThemeSeed): WebThemeSeed {
  if (theme === "dark") return "zai-dark";
  if (theme === "light") return "zai-light";
  return theme;
}

export function resolveWebInitialTheme({
  storedTheme,
  defaultTheme = WEB_DEFAULT_THEME,
}: {
  storedTheme?: string | null;
  defaultTheme?: WebThemeSeed;
}): WebThemeSeed {
  if (isWebThemeSeed(storedTheme)) {
    return normalizeWebThemeSeed(storedTheme);
  }

  return normalizeWebThemeSeed(defaultTheme);
}

/**
 * 把主题偏好落到 document 根节点。主应用入口（`main.tsx`）与独立远程控制入口
 * （`v3/entry.tsx`）共用这一份，避免两处各写一遍类名切换而出现主题差异。
 */
export function applyWebThemeToDocument(
  doc: Document = document,
  storage: Pick<Storage, "getItem"> = localStorage,
): void {
  const saved = resolveWebInitialTheme({ storedTheme: storage.getItem("mode-theme") });
  const prefersDark = doc.defaultView?.matchMedia("(prefers-color-scheme: dark)").matches ?? false;
  const dark = saved === "system" ? prefersDark : saved === "dark" || saved === "zai-dark";
  const appliedTheme =
    saved === "system" ? (dark ? "zai-dark" : "zai-light") : normalizeWebThemeSeed(saved);
  doc.documentElement.classList.toggle("dark", dark);
  doc.documentElement.classList.toggle("theme-zai-light", appliedTheme === "zai-light");
  doc.documentElement.classList.toggle("theme-zai-dark", appliedTheme === "zai-dark");
}
