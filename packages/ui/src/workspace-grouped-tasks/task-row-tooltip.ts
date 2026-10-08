import type { ModeTaskChangeSummary } from "@mode/shared";

export function formatGroupedTaskHoverChangeParts(
  summary: ModeTaskChangeSummary | null,
): string[] {
  if (!summary) {
    return [];
  }

  const parts: string[] = [];
  if (summary.added > 0) {
    parts.push(`+${summary.added}`);
  }
  if (summary.removed > 0) {
    parts.push(`-${summary.removed}`);
  }
  return parts;
}
