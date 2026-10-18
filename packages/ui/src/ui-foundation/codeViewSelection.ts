// CodeView 行选区与评论草稿的纯逻辑（spec: §2.20）。
export interface CodeViewSelection {
  startLine: number;
  endLine: number;
}

export function normalizeCodeViewSelection(anchor: number, focus: number): CodeViewSelection {
  return anchor <= focus
    ? { startLine: anchor, endLine: focus }
    : { startLine: focus, endLine: anchor };
}

export function isCodeViewLineSelected(
  selection: CodeViewSelection | null,
  lineNumber: number,
): boolean {
  return selection !== null && lineNumber >= selection.startLine && lineNumber <= selection.endLine;
}

export function getCodeViewSelectedText(
  lines: readonly string[],
  selection: CodeViewSelection | null,
  firstLineNumber: number,
): string {
  if (!selection) {
    return "";
  }
  const startIndex = Math.max(0, selection.startLine - firstLineNumber);
  const endIndex = Math.min(lines.length - 1, selection.endLine - firstLineNumber);
  if (endIndex < startIndex) {
    return "";
  }
  return lines.slice(startIndex, endIndex + 1).join("\n");
}
