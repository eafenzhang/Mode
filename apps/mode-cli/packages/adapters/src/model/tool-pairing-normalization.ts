import type { ModelInputMessage } from "@mode/contracts";
import { getStatusCode, unwrapRetryError } from "./failure-inspection.js";

// tool_use / tool_result 配对自愈（spec: docs/specs/provider-history-tool-pairing.md）。
//
// 背景：deepseek（Anthropic 兼容端点）曾对历史中的孤儿 tool_result 返回
// 400 retryable=false（"Each `tool_result` block must have a corresponding
// `tool_use` block in the previous message"），整个 turn 硬失败。全链路审计后
// 当前写入路径均保持配对，但同族缺陷历史上已复发过两次，且仓库没有任何边界
// 校验；本模块与 thinking 签名修复（reasoning-history-normalization）同机制：
// provider 明确拒绝后，只修本次请求副本并给一次不占预算的重试，不回写
// canonical history，不改写任何健康请求。

export interface ToolPairingRepairResult {
  /** 修复后的请求副本；调用方仅用于本次物理请求，不得写回 canonical history。 */
  messages: ModelInputMessage[];
  /** 被重新锚定到声明方 assistant 之后的 toolCallId（按原始顺序）。 */
  reanchoredToolCallIds: string[];
  /** 被丢弃的孤儿 / 重复 toolCallId（按原始顺序）。 */
  droppedToolCallIds: string[];
}

/** Anthropic 风格配对语法错误的窄化匹配；其余 invalid_request 不碰历史。 */
export function isToolResultPairingRejection(error: unknown): boolean {
  const unwrapped = unwrapRetryError(error);
  if (getStatusCode(unwrapped) !== 400) return false;
  const message = errorMessage(unwrapped).toLowerCase();
  return (
    message.includes("tool_result") &&
    message.includes("tool_use") &&
    (message.includes("corresponding") || message.includes("previous message"))
  );
}

export function repairToolResultPairingAfterInvalidRequest(
  messages: readonly ModelInputMessage[],
  error: unknown,
): ToolPairingRepairResult | undefined {
  if (!isToolResultPairingRejection(error)) return undefined;
  // 匹配成功但没有孤儿时不产生修复副本：no-op 不得消耗唯一的重试机会。
  return repairOrphanToolResults(messages);
}

/**
 * 按 wire 分组语义修复孤儿 tool_result：
 * - 配对成立（向前越过相邻 user/tool 后停在声明了该 id 的 assistant）→ 保留；
 * - id 已有配对成立的结果 → 后来的重复条目丢弃；
 * - 声明方存在于更早位置但被 system 等隔断 → 重新锚定到声明方紧后；
 * - 找不到任何声明方 → 丢弃（tool_use 本身已缺失，保留只会换来同样的 400）。
 * 无孤儿时返回 undefined。
 */
export function repairOrphanToolResults(
  messages: readonly ModelInputMessage[],
): ToolPairingRepairResult | undefined {
  const droppedIndexes: number[] = [];
  const moved: Array<{ anchorIndex: number; messageIndex: number; toolCallId: string }> = [];
  // 逐个按原始顺序处理：先出现的合法结果占住 id，后来的同 id 结果按重复丢弃。
  const fulfilledToolCallIds = new Set<string>();

  for (let index = 0; index < messages.length; index++) {
    const message = messages[index]!;
    if (message.role !== "tool" || !message.toolCallId) continue;
    const toolCallId = message.toolCallId;
    if (fulfilledToolCallIds.has(toolCallId)) {
      droppedIndexes.push(index);
      continue;
    }

    const boundary = messages[findPairingBoundary(messages, index)];
    if (boundary?.role === "assistant" && declaresToolCall(boundary, toolCallId)) {
      fulfilledToolCallIds.add(toolCallId);
      continue;
    }

    const anchorIndex = findDeclaringAssistant(messages, index - 1, toolCallId);
    if (anchorIndex < 0) {
      droppedIndexes.push(index);
      continue;
    }
    fulfilledToolCallIds.add(toolCallId);
    moved.push({ anchorIndex, messageIndex: index, toolCallId });
  }

  if (droppedIndexes.length === 0 && moved.length === 0) return undefined;

  const movedByAnchor = new Map<number, typeof moved>();
  for (const item of moved) {
    const bucket = movedByAnchor.get(item.anchorIndex);
    if (bucket) bucket.push(item);
    else movedByAnchor.set(item.anchorIndex, [item]);
  }
  const skipped = new Set([...droppedIndexes, ...moved.map((item) => item.messageIndex)]);

  const repaired: ModelInputMessage[] = [];
  for (let index = 0; index < messages.length; index++) {
    if (skipped.has(index)) continue;
    repaired.push(messages[index]!);
    for (const item of movedByAnchor.get(index) ?? []) {
      repaired.push(messages[item.messageIndex]!);
    }
  }

  return {
    messages: repaired,
    reanchoredToolCallIds: moved.map((item) => item.toolCallId),
    droppedToolCallIds: droppedIndexes.map((index) => messages[index]!.toolCallId!),
  };
}

/**
 * 配对边界与 Anthropic groupIntoBlocks 一致：向前越过相邻的 user/tool
 * （wire 上它们与结果合并进同一个 user block），停在 assistant 或 system。
 * 停在 system 说明中间被隔断，前一条 wire 消息已不再是声明方 → 视为孤儿。
 */
function findPairingBoundary(
  messages: readonly ModelInputMessage[],
  toolMessageIndex: number,
): number {
  let index = toolMessageIndex - 1;
  while (index >= 0) {
    const role = messages[index]!.role;
    if (role !== "user" && role !== "tool") break;
    index -= 1;
  }
  return index;
}

function findDeclaringAssistant(
  messages: readonly ModelInputMessage[],
  fromIndex: number,
  toolCallId: string,
): number {
  for (let index = fromIndex; index >= 0; index--) {
    const message = messages[index]!;
    if (message.role === "assistant" && declaresToolCall(message, toolCallId)) return index;
  }
  return -1;
}

function declaresToolCall(message: ModelInputMessage, toolCallId: string): boolean {
  return message.toolCalls?.some((toolCall) => toolCall.id === toolCallId) ?? false;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message?: unknown }).message ?? "");
  }
  return String(error ?? "");
}
