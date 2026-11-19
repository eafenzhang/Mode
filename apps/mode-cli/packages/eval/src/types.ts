// LT-4 数据形状：Case/Suite 与 oracle 行为夹具同形（spec §4）。
// 只校验 eval 需要的字段；夹具里未来新增的键透传，避免一份新夹具让整个 suite 拒绝加载。
import { z } from "zod";

export const ScriptToolCallSchema = z.object({
  name: z.string().min(1),
  input: z.unknown(),
});

export const ScriptStepSchema = z.union([
  z.object({ text: z.string() }).strict(),
  z.object({ toolCalls: z.array(ScriptToolCallSchema).min(1) }).strict(),
]);

export const EvalEventSchema = z.object({
  type: z.string().min(1),
  payload: z.unknown(),
});

export const ToolCallSchema = z.object({
  name: z.string(),
  input: z.unknown(),
  ok: z.boolean(),
  output: z.string(),
});

export const ObservedSchema = z.object({
  toolCalls: z.array(ToolCallSchema),
  files: z.record(z.string()),
  events: z.array(EvalEventSchema),
});

export const GeneratedSeedSchema = z.object({
  char: z.string().min(1),
  width: z.number().int().nonnegative(),
  lines: z.number().int().nonnegative(),
});

export const FollowUpTurnSchema = z.object({
  prompt: z.string(),
  calls: z.number().int().nonnegative(),
});

export const CaseSchema = z
  .object({
    case: z.string().min(1),
    prompt: z.string(),
    seed: z.record(z.string()).optional(),
    seedBytesHex: z.record(z.string()).optional(),
    generatedSeed: z.record(GeneratedSeedSchema).optional(),
    script: z.array(ScriptStepSchema).min(1),
    followUpTurns: z.array(FollowUpTurnSchema).optional(),
    expect: ObservedSchema.optional(),
    timeoutMs: z.number().int().positive().optional(),
  })
  .passthrough();

export const SuiteSchema = z.object({
  schemaVersion: z.literal(1),
  name: z.string().optional(),
  cases: z.array(CaseSchema).min(1),
});

export type ScriptToolCall = z.infer<typeof ScriptToolCallSchema>;
export type ScriptStep = z.infer<typeof ScriptStepSchema>;
export type EvalEvent = z.infer<typeof EvalEventSchema>;
export type ToolCall = z.infer<typeof ToolCallSchema>;
export type Observed = z.infer<typeof ObservedSchema>;
export type GeneratedSeed = z.infer<typeof GeneratedSeedSchema>;
export type FollowUpTurn = z.infer<typeof FollowUpTurnSchema>;
export type EvalCase = z.infer<typeof CaseSchema>;
export type Suite = z.infer<typeof SuiteSchema>;

export const TRAJECTORY_SCHEMA_VERSION = 1 as const;

export type Mismatch = {
  path: string;
  expected: unknown;
  actual: unknown;
};

export type CaseStatus = "passed" | "failed" | "recorded";

export type CaseResult = {
  case: string;
  status: CaseStatus;
  /** 本次 case 的实际耗时；确定性比较时剔除。 */
  durationMs: number;
  toolCalls: string[];
  turns: number;
  mismatches: Mismatch[];
  trajectoryPath: string;
  /** case 抛错时的错误消息（已脱敏）；正常完成时缺省。 */
  error?: string;
  stderr?: string;
};

export type RunReport = {
  schemaVersion: 1;
  suite: string;
  totals: {
    cases: number;
    passed: number;
    failed: number;
    recorded: number;
  };
  results: CaseResult[];
};
