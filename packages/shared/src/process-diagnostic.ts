import { z } from "zod";

// 启动早期或协议故障时 stdout 尚不可用，进程诊断使用独立的 stderr 单行契约。
export const MODE_PROCESS_DIAGNOSTIC_PREFIX = "[mode-process-exception] ";
export const MODE_PROCESS_DIAGNOSTIC_NAME_MAX_CHARS = 128;
export const MODE_PROCESS_DIAGNOSTIC_MESSAGE_MAX_CHARS = 4_000;
export const MODE_PROCESS_DIAGNOSTIC_STACK_MAX_CHARS = 16_000;
export const MODE_PROCESS_DIAGNOSTIC_MAX_LINE_CHARS = 128 * 1024;
export const MODE_AGENT_LIFECYCLE_LOG_MARKER = "[mode-agent-lifecycle-reported]";

const processErrorKindSchema = z.enum(["uncaughtException", "unhandledRejection"]);
export const modeProcessDiagnosticSchema = z
  .object({
    version: z.literal(1),
    errorId: z.uuid(),
    kind: processErrorKindSchema,
    origin: processErrorKindSchema,
    name: z.string().min(1).max(MODE_PROCESS_DIAGNOSTIC_NAME_MAX_CHARS),
    message: z.string().max(MODE_PROCESS_DIAGNOSTIC_MESSAGE_MAX_CHARS),
    stack: z.string().max(MODE_PROCESS_DIAGNOSTIC_STACK_MAX_CHARS).optional(),
    occurredAt: z.number().int().nonnegative(),
  })
  .strict();
export type ModeProcessDiagnostic = z.infer<typeof modeProcessDiagnosticSchema>;

export function parseModeProcessDiagnostic(line: string): ModeProcessDiagnostic | undefined {
  if (
    !line.startsWith(MODE_PROCESS_DIAGNOSTIC_PREFIX) ||
    line.length > MODE_PROCESS_DIAGNOSTIC_MAX_LINE_CHARS
  ) {
    return undefined;
  }
  try {
    const result = modeProcessDiagnosticSchema.safeParse(
      JSON.parse(line.slice(MODE_PROCESS_DIAGNOSTIC_PREFIX.length)),
    );
    return result.success ? result.data : undefined;
  } catch {
    // 诊断旁路不得因损坏帧中断业务协议或退出处理。
    return undefined;
  }
}
