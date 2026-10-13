import type { AssistantTextRow } from "@zcode/shared/zcode-protocol-v4";
import {
  AssistantPreviewCard,
  type AssistantPreviewCard,
} from "@/lib/assistantPreviewCards.js";

export interface BuildAssistantPreviewCardsForAssistantTextRowParams {
  row: AssistantTextRow;
  assistantTextRows: readonly AssistantTextRow[];
  latestAssistantTextRow?: AssistantTextRow;
  workspacePath: string;
  changedFilePaths?: readonly string[];
}

function isPreviewCardTerminalState(state: AssistantTextRow["state"]): boolean {
  return state === "complete" || state === "interrupted";
}

export function buildAssistantPreviewCardsForAssistantTextRow({
  row,
  assistantTextRows,
  latestAssistantTextRow,
  workspacePath,
  changedFilePaths,
}: BuildAssistantPreviewCardsForAssistantTextRowParams): AssistantPreviewCard[] {
  if (
    !latestAssistantTextRow ||
    row.rowId !== latestAssistantTextRow.rowId ||
    !isPreviewCardTerminalState(row.state)
  ) {
    return [];
  }

  const turnText = assistantTextRows
    .map((assistantRow) => assistantRow.text)
    .filter((text) => text.trim().length > 0)
    .join("\n\n");

  return AssistantPreviewCard(turnText, workspacePath, {
    changedFilePaths,
  });
}
