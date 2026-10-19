// DiffViewer 的纯计算部分（与 @pierre/diffs 解耦，便于单测）。
import type { CSSProperties } from "react";
import type { BundledTheme } from "shiki";
import type { FileDiffOptions } from "@pierre/diffs";
import { cn } from "@/components/lib/utils.js";

export interface DiffViewerThemeInput {
  options?: FileDiffOptions<undefined>;
  lightTheme?: BundledTheme;
  darkTheme?: BundledTheme;
  themeType?: "light" | "dark";
}

export function buildDiffViewerOptions({
  options,
  lightTheme,
  darkTheme,
  themeType,
}: DiffViewerThemeInput): FileDiffOptions<undefined> | undefined {
  if (!lightTheme && !darkTheme && !themeType) {
    return options;
  }
  const theme =
    lightTheme && darkTheme
      ? { light: lightTheme, dark: darkTheme }
      : (lightTheme ?? darkTheme ?? options?.theme);
  return {
    ...options,
    ...(theme ? { theme } : {}),
    ...(themeType ? { themeType } : {}),
  } as FileDiffOptions<undefined>;
}

export interface DiffViewerStyleInput {
  style?: CSSProperties;
  fontSizePx?: number;
}

export function buildDiffViewerStyle({ style, fontSizePx }: DiffViewerStyleInput): CSSProperties {
  return {
    ...style,
    "--diffs-font-family": "var(--font-mono)",
    "--diffs-bg": "var(--color-background)",
    "--diffs-light-bg": "var(--color-background)",
    "--diffs-dark-bg": "var(--color-background)",
    ...(fontSizePx ? { "--diffs-font-size": `${fontSizePx}px` } : {}),
  } as CSSProperties;
}

export function diffViewerContainerClassName(className?: string): string {
  return cn(["min-w-0 overflow-auto", className]);
}
