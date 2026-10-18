// CodeView 的行模型（无 UI/高亮依赖，便于单测）。
export interface CodeViewRange {
  startLine: number;
  endLine: number;
}

export interface CodeViewLineModel {
  index: number;
  number: number;
  marked: boolean;
  focused: boolean;
}

export function buildCodeViewLineModels(options: {
  lineCount: number;
  firstLineNumber: number;
  markedLines?: readonly number[];
  focusedRange?: CodeViewRange | null;
}): CodeViewLineModel[] {
  const { lineCount, firstLineNumber, markedLines, focusedRange } = options;
  const marked = new Set(markedLines ?? []);
  return Array.from({ length: Math.max(0, lineCount) }, (_, index) => {
    const number = firstLineNumber + index;
    const focused =
      focusedRange != null && number >= focusedRange.startLine && number <= focusedRange.endLine;
    return { index, number, marked: marked.has(number), focused };
  });
}
