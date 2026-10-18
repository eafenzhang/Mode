// Lightweight diff 的行契约（无 UI/高亮依赖，便于单测）。
import { getLightweightDiffLineParts } from "./LightweightDiffPreview.js";

export function getHighlightedLightweightDiffLine(line: string): {
  code: string;
  marker: string;
} {
  const parts = getLightweightDiffLineParts(line);
  return { code: parts.content, marker: parts.marker };
}

export function buildHighlightedLightweightDiffCode(lines: readonly string[]): string {
  return lines.map((line) => getHighlightedLightweightDiffLine(line).code).join("\n");
}
