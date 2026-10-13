import { z } from "zod";

export const modeProtocolTraceSchema = z
  .object({
    traceId: z.string().min(1).optional(),
    parentId: z.string().min(1).optional(),
    spanId: z.string().min(1).optional(),
    traceparent: z.string().min(1).optional(),
  })
  .strict();
export type ModeProtocolTrace = z.infer<typeof modeProtocolTraceSchema>;
