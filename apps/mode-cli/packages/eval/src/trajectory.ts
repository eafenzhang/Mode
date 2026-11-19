// 轨迹 JSONL（spec §5）：每行一个 JSON 对象；kind = meta|event|toolCall|files|result。
// 写盘前已完成规范化与脱敏；去掉 timing 后逐字节稳定。
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname } from "node:path";

import type {
  EvalCase,
  FollowUpTurn,
  Observed,
  ScriptStep,
} from "./types.js";
import { TRAJECTORY_SCHEMA_VERSION } from "./types.js";

export type TrajectoryMeta = {
  kind: "meta";
  schemaVersion: typeof TRAJECTORY_SCHEMA_VERSION;
  case: string;
  prompt: string;
  seed?: Record<string, string>;
  seedBytesHex?: Record<string, string>;
  generatedSeed?: Record<string, { char: string; width: number; lines: number }>;
  script: ScriptStep[];
  followUpTurns?: FollowUpTurn[];
};

export type TrajectoryLine =
  | TrajectoryMeta
  | { kind: "event"; type: string; payload: unknown }
  | { kind: "toolCall"; name: string; input: unknown; ok: boolean; output: string }
  | { kind: "files"; files: Record<string, string> }
  | {
      kind: "result";
      status: "passed" | "failed" | "recorded";
      mismatches: Array<{ path: string; expected: unknown; actual: unknown }>;
      error?: string;
    };

export type Trajectory = {
  meta: TrajectoryMeta;
  events: Array<{ type: string; payload: unknown }>;
  toolCalls: Array<{ name: string; input: unknown; ok: boolean; output: string }>;
  files: Record<string, string>;
  result?: TrajectoryLine & { kind: "result" };
};

export async function writeTrajectory(path: string, trajectory: Trajectory): Promise<void> {
  const lines: string[] = [];
  lines.push(JSON.stringify(trajectory.meta));
  for (const event of trajectory.events) {
    lines.push(JSON.stringify({ kind: "event", type: event.type, payload: event.payload } satisfies TrajectoryLine));
  }
  for (const call of trajectory.toolCalls) {
    lines.push(JSON.stringify({ kind: "toolCall", ...call } satisfies TrajectoryLine));
  }
  lines.push(JSON.stringify({ kind: "files", files: trajectory.files } satisfies TrajectoryLine));
  if (trajectory.result) {
    lines.push(JSON.stringify(trajectory.result));
  }
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${lines.join("\n")}\n`, "utf8");
}

export async function readTrajectory(path: string): Promise<Trajectory> {
  const text = await readFile(path, "utf8");
  let meta: TrajectoryMeta | undefined;
  const events: Array<{ type: string; payload: unknown }> = [];
  const toolCalls: Array<{ name: string; input: unknown; ok: boolean; output: string }> = [];
  let files: Record<string, string> = {};
  let result: (TrajectoryLine & { kind: "result" }) | undefined;
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const parsed = JSON.parse(trimmed) as TrajectoryLine;
    if (parsed.kind === "meta") {
      meta = parsed;
      continue;
    }
    if (parsed.kind === "event") events.push({ type: parsed.type, payload: parsed.payload });
    else if (parsed.kind === "toolCall")
      toolCalls.push({ name: parsed.name, input: parsed.input, ok: parsed.ok, output: parsed.output });
    else if (parsed.kind === "files") files = parsed.files;
    else if (parsed.kind === "result") result = parsed;
  }
  if (!meta) throw new Error(`trajectory has no meta line: ${path}`);
  return { meta, events, toolCalls, files, result };
}

/** 转成 oracle 行为夹具形状（`packages/desktop/tests/agent-runtime-oracle` 的输入格式）。 */
export function toOracleFixture(trajectory: Trajectory): EvalCase & { expect: Observed } {
  const meta = trajectory.meta;
  const fixture: EvalCase & { expect: Observed } = {
    case: meta.case,
    prompt: meta.prompt,
    seed: meta.seed ?? {},
    script: meta.script,
    expect: {
      toolCalls: trajectory.toolCalls,
      files: trajectory.files,
      events: trajectory.events.map((event) => ({ type: event.type, payload: event.payload })),
    },
  };
  if (meta.seedBytesHex) fixture.seedBytesHex = meta.seedBytesHex;
  if (meta.generatedSeed) fixture.generatedSeed = meta.generatedSeed;
  if (meta.followUpTurns) fixture.followUpTurns = meta.followUpTurns;
  return fixture;
}

export function trajectoryFileName(caseId: string): string {
  // case id 可能含任意字符；文件名只保留安全字符，其余折叠成 `_`。
  const safe = caseId.replace(/[^A-Za-z0-9._-]+/g, "_");
  return `${safe}.jsonl`;
}
