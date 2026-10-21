// ============================================================
// SendFile Tool Handler
// ============================================================

import {
  CoreErrorType,
  SEND_FILE_TOOL_NAME,
  SendFileInputJsonSchema,
  SendFileInputSchema,
  SendFileOutputJsonSchema,
  SendFileOutputSchema,
  createCoreError,
  type SendFileInput,
  type SendFileOutput,
  type ToolPermissionSpec,
} from "@mode/contracts";
import type { ToolEntry, ToolExecutionContext, ToolHandler } from "../types.js";

/** 50MB 文件经分片上传可能需要数十秒；给足单次调用的时间窗。 */
const SEND_FILE_TOOL_TIMEOUT_MS = 60_000;
const SEND_FILE_MODEL_BYTES = 8_000;

function sendFilePermission(
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

function assertBotMediaPort(
  context: ToolExecutionContext,
): asserts context is ToolExecutionContext & {
  botMediaPort: NonNullable<ToolExecutionContext["botMediaPort"]>;
} {
  if (context.botMediaPort) return;
  // 端口只在 Host 确认本轮带 botDeliveryTarget 时注入；缺失说明会话不是 bot 回合，
  // 或 Host 版本过旧。给出可读错误，不触碰任何发送路径。
  throw createCoreError(
    CoreErrorType.ConfigurationError,
    `${SEND_FILE_TOOL_NAME} is only available in an IM bot conversation.`,
    {
      context: {
        toolCallId: context.toolCallId,
        toolName: SEND_FILE_TOOL_NAME,
      },
      recoverable: false,
      retryable: false,
    },
  );
}

const sendFileHandler: ToolHandler = async (input, context) => {
  const parsed = SendFileInputSchema.parse(input) as SendFileInput;
  assertBotMediaPort(context);
  const result = await context.botMediaPort.sendMedia({
    filePath: parsed.path,
    ...(parsed.caption ? { caption: parsed.caption } : {}),
  });
  return {
    status: "sent",
    filename: result.filename,
    kind: result.kind,
  } satisfies SendFileOutput;
};

export const sendFileToolEntry: ToolEntry = {
  capability: "Send a workspace file to the current IM bot conversation",
  metadata: {
    name: SEND_FILE_TOOL_NAME,
    description:
      "Send a file from the current workspace to the chat where this IM bot conversation is happening. Use this when the user asks you to send, share, or hand over a generated file. The destination is fixed by the host; you cannot choose another chat.",
    modelInstructions: [
      "Use this only when the user asks for a file to be sent or shared in this chat.",
      "Pass a workspace-relative path when possible; absolute paths are accepted only inside the workspace, the app data directory, or the system temp directory.",
      "Do not use this to send secrets, credentials, or files the user did not ask to share.",
      "Optionally pass a short caption; it is shown with the file when the platform supports it.",
    ],
    readOnly: false,
    destructive: false,
    // 同一会话的媒体发送由 Host 侧 per-bot 队列串行化；工具本身不并行提交。
    concurrentSafe: false,
    timeoutMs: SEND_FILE_TOOL_TIMEOUT_MS,
    maxOutputBytes: SEND_FILE_MODEL_BYTES,
    sideEffectScope: "network",
    riskLevel: "medium",
    // 与用户手动 /send 对齐：这是用户显式要求的消息级动作，不额外弹权限。
    needsApproval: false,
  },
  handler: sendFileHandler,
  inputSchema: SendFileInputJsonSchema,
  outputSchema: SendFileOutputJsonSchema,
  runtimeInputSchema: SendFileInputSchema,
  runtimeOutputSchema: SendFileOutputSchema,
  permission: sendFilePermission(
    "bot.media.send",
    "SendFile uploads a workspace file to the current IM chat",
    false,
  ),
  resultBudget: {
    maxInlineBytes: SEND_FILE_MODEL_BYTES,
    maxModelBytes: SEND_FILE_MODEL_BYTES,
    strategy: "truncate",
    preview: {
      maxBytes: SEND_FILE_MODEL_BYTES,
      direction: "head",
    },
  },
  timeout: {
    defaultMs: SEND_FILE_TOOL_TIMEOUT_MS,
    maxMs: SEND_FILE_TOOL_TIMEOUT_MS,
    allowCallOverride: false,
  },
  cancellation: {
    supported: true,
    cleanup: "none",
    userVisibleMessage: "SendFile was cancelled before the file was delivered",
  },
  trace: {
    required: true,
    propagateToAdapters: true,
    recordInput: "summary",
    recordOutput: "summary",
  },
};
