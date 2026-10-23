// ============================================================
// HandoffMessage Tool Handler
// ============================================================

import {
  CoreErrorType,
  HANDOFF_MESSAGE_TOOL_NAME,
  HandoffMessageInputJsonSchema,
  HandoffMessageInputSchema,
  HandoffMessageOutputJsonSchema,
  HandoffMessageOutputSchema,
  createCoreError,
  type HandoffMessageInput,
  type HandoffMessageOutput,
  type ToolPermissionSpec,
} from "@mode/contracts";
import type { ToolEntry, ToolExecutionContext, ToolHandler } from "../types.js";

/** 跨聊天交接要等 Host 侧 botsService 完成目标归属校验与投递，给足单次调用的时间窗。 */
const HANDOFF_MESSAGE_TOOL_TIMEOUT_MS = 60_000;
const HANDOFF_MESSAGE_MODEL_BYTES = 8_000;

function handoffPermission(
  permission: string,
  reason: string,
  needsApproval: boolean,
): ToolPermissionSpec {
  return {
    permission,
    reason,
    riskLevel: "medium",
    sideEffectScope: "network",
    needsApproval,
    patternSources: ["toolName", "input"],
    alwaysAllowPatternSources: ["toolName"],
    denyPriority: "beforeAsk",
  };
}

function assertBotHandoffPort(
  context: ToolExecutionContext,
): asserts context is ToolExecutionContext & {
  botHandoffPort: NonNullable<ToolExecutionContext["botHandoffPort"]>;
} {
  if (context.botHandoffPort) return;
  // 端口只在 Host 确认本轮带 botDeliveryTarget 时注入；缺失说明会话不是 bot 回合，
  // 或 Host 版本过旧。给出可读错误，不触碰任何投递路径。
  throw createCoreError(
    CoreErrorType.ConfigurationError,
    `${HANDOFF_MESSAGE_TOOL_NAME} is only available in an IM bot conversation.`,
    {
      context: {
        toolCallId: context.toolCallId,
        toolName: HANDOFF_MESSAGE_TOOL_NAME,
      },
      recoverable: false,
      retryable: false,
    },
  );
}

const handoffMessageHandler: ToolHandler = async (input, context) => {
  const parsed = HandoffMessageInputSchema.parse(input) as HandoffMessageInput;
  assertBotHandoffPort(context);
  const result = await context.botHandoffPort.handoff({
    ...parsed,
    toolCallId: context.toolCallId,
  });
  return {
    status: result.status,
    target: result.target,
  } satisfies HandoffMessageOutput;
};

export const handoffMessageToolEntry: ToolEntry = {
  capability: "Hand off a message to another chat of the current IM bot",
  metadata: {
    name: HANDOFF_MESSAGE_TOOL_NAME,
    description:
      "Hand a message over to another chat of the SAME IM bot (for example another private chat or a group). The target must be one of the conversation keys listed in your conversation context; the target chat's own session receives the handoff and replies with its own context. Use this when the user explicitly asks you to pass, forward, or hand over a message to a different chat of this bot.",
    modelInstructions: [
      "Use this only when the user asks for a message to be passed, forwarded, or handed over to another chat of the current bot.",
      "The target must come from the conversation list injected into this turn's context; targets outside that list are rejected by the host.",
      "Optionally pass short background (context) so the target chat can pick up the task with its own context, and workspace file paths as attachments when the user asked to include files.",
      "Do not use this to answer the user directly in the current chat, and do not guess target keys that were not listed.",
    ],
    readOnly: false,
    destructive: false,
    // 同一会话的消息投递由 Host 侧 per-bot 队列串行化；工具本身不并行提交。
    concurrentSafe: false,
    timeoutMs: HANDOFF_MESSAGE_TOOL_TIMEOUT_MS,
    maxOutputBytes: HANDOFF_MESSAGE_MODEL_BYTES,
    sideEffectScope: "network",
    riskLevel: "medium",
    // 与用户手动转达对齐（spec R1）：这是用户显式要求的消息级动作，不额外弹权限。
    needsApproval: false,
  },
  handler: handoffMessageHandler,
  inputSchema: HandoffMessageInputJsonSchema,
  outputSchema: HandoffMessageOutputJsonSchema,
  runtimeInputSchema: HandoffMessageInputSchema,
  runtimeOutputSchema: HandoffMessageOutputSchema,
  permission: handoffPermission(
    "bot.handoff.send",
    "HandoffMessage delivers a message to another chat of the current IM bot",
    false,
  ),
  resultBudget: {
    maxInlineBytes: HANDOFF_MESSAGE_MODEL_BYTES,
    maxModelBytes: HANDOFF_MESSAGE_MODEL_BYTES,
    strategy: "truncate",
    preview: {
      maxBytes: HANDOFF_MESSAGE_MODEL_BYTES,
      direction: "head",
    },
  },
  timeout: {
    defaultMs: HANDOFF_MESSAGE_TOOL_TIMEOUT_MS,
    maxMs: HANDOFF_MESSAGE_TOOL_TIMEOUT_MS,
    allowCallOverride: false,
  },
  cancellation: {
    supported: true,
    cleanup: "none",
    userVisibleMessage: "HandoffMessage was cancelled before the target chat received the message",
  },
  trace: {
    required: true,
    propagateToAdapters: true,
    recordInput: "summary",
    recordOutput: "summary",
  },
};
