// Runner（spec §2/§5/§6）：每个 case 一个隔离根与 workspace；投影 → 脱敏 → 比较 → 轨迹/报告。
// 用例串行执行，保证隔离与确定性；时间字段不进轨迹，报告的 durationMs 由确定性测试剔除。
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { performance } from "node:perf_hooks";

import type { SessionFactory } from "./agent-server.js";
import { normalizeValue, projectEventStream, projectToolCalls, stableStringify, type NormalizeContext } from "./normalize.js";
import type { Redactor } from "./redact.js";
import { buildReport } from "./report.js";
import { takeFileSnapshot } from "./snapshot.js";
import { readTrajectory, trajectoryFileName, writeTrajectory, type Trajectory } from "./trajectory.js";
import type { CaseResult, EvalCase, Mismatch, ScriptStep, Suite } from "./types.js";

export type RunSuiteOptions = {
  suite: Suite;
  /** suite 的绝对路径，仅用于报告展示。 */
  suitePath: string;
  outDir: string;
  sessionFactory: SessionFactory;
  redactor: Redactor;
  defaultTimeoutMs?: number;
};

export async function runSuite(options: RunSuiteOptions): Promise<ReturnType<typeof buildReport>> {
  const results: CaseResult[] = [];
  const trajectoriesRoot = join(options.outDir, "trajectories");
  await mkdir(trajectoriesRoot, { recursive: true });
  const defaultTimeoutMs = options.defaultTimeoutMs ?? 120_000;

  for (const evalCase of options.suite.cases) {
    const result = await runSingleCase(evalCase, {
      sessionFactory: options.sessionFactory,
      redactor: options.redactor,
      timeoutMs: evalCase.timeoutMs ?? defaultTimeoutMs,
      trajectoriesRoot,
    });
    results.push(result);
  }
  return buildReport(options.suitePath, results);
}

export async function runSingleCase(
  evalCase: EvalCase,
  options: {
    sessionFactory: SessionFactory;
    redactor: Redactor;
    timeoutMs: number;
    trajectoriesRoot: string;
  },
): Promise<CaseResult> {
  const started = performance.now();
  const tmpRoot = await mkdtemp(join(tmpdir(), "mode-eval-"));
  const cwd = join(tmpRoot, "workspace");
  const dataRoot = join(tmpRoot, "data");
  await mkdir(cwd, { recursive: true });
  await mkdir(dataRoot, { recursive: true });
  const trajectoryPath = join(options.trajectoriesRoot, trajectoryFileName(evalCase.case));
  const context: NormalizeContext = { cwd, dataRoot, tmpDir: tmpdir() };

  try {
    await writeSeeds(cwd, evalCase);
    const scriptPath = join(dataRoot, "script.json");
    await writeFile(
      scriptPath,
      JSON.stringify({ case: evalCase.case, script: substituteCwd(evalCase.script, cwd) }),
      "utf8",
    );

    const session = await options.sessionFactory.start({
      case: evalCase,
      scriptPath,
      cwd,
      dataRoot,
      timeoutMs: options.timeoutMs,
    });
    try {
      await session.send(evalCase.prompt);
      for (const turn of evalCase.followUpTurns ?? []) {
        await session.send(turn.prompt);
      }
    } finally {
      await session.stop();
    }

    const observed = await observeSession(session.events, context, options.redactor);
    const turns = session.events.filter((event) => isTerminal(event.type)).length;
    const mismatches = evalCase.expect
      ? compareObserved(evalCase.expect, observed, MAX_MISMATCHES)
      : [];
    const status: CaseResult["status"] = !evalCase.expect
      ? "recorded"
      : mismatches.length > 0
        ? "failed"
        : "passed";

    const trajectory: Trajectory = {
      meta: {
        kind: "meta",
        schemaVersion: 1,
        case: evalCase.case,
        prompt: evalCase.prompt,
        seed: evalCase.seed,
        seedBytesHex: evalCase.seedBytesHex,
        generatedSeed: evalCase.generatedSeed,
        script: evalCase.script,
        followUpTurns: evalCase.followUpTurns,
      },
      events: observed.events,
      toolCalls: observed.toolCalls,
      files: observed.files,
      result: {
        kind: "result",
        status,
        mismatches,
      },
    };
    await writeTrajectory(trajectoryPath, trajectory);

    return {
      case: evalCase.case,
      status,
      durationMs: Math.round(performance.now() - started),
      toolCalls: observed.toolCalls.map((call) => call.name),
      turns,
      mismatches,
      trajectoryPath,
      stderr: session.stderr.trim() || undefined,
    };
  } catch (error) {
    const message = options.redactor.redactString(
      error instanceof Error ? error.message : String(error),
    );
    await writeTrajectory(trajectoryPath, {
      meta: {
        kind: "meta",
        schemaVersion: 1,
        case: evalCase.case,
        prompt: evalCase.prompt,
        seed: evalCase.seed,
        seedBytesHex: evalCase.seedBytesHex,
        generatedSeed: evalCase.generatedSeed,
        script: evalCase.script,
        followUpTurns: evalCase.followUpTurns,
      },
      events: [],
      toolCalls: [],
      files: {},
      result: { kind: "result", status: "failed", mismatches: [], error: message },
    });
    return {
      case: evalCase.case,
      status: "failed",
      durationMs: Math.round(performance.now() - started),
      toolCalls: [],
      turns: 0,
      mismatches: [],
      trajectoryPath,
      error: message,
    };
  } finally {
    await rm(tmpRoot, { recursive: true, force: true }).catch(() => {});
  }
}

/** 投影 + 脱敏：先按 §6 规范化（去易变字段），再按 §7 脱敏。 */
async function observeSession(
  events: readonly { type: string; payload: unknown }[],
  context: NormalizeContext,
  redactor: Redactor,
): Promise<{ toolCalls: Trajectory["toolCalls"]; files: Record<string, string>; events: Trajectory["events"] }> {
  const rawEvents = events.map((event) => ({ type: event.type, payload: event.payload }));
  const projectedEvents = projectEventStream(rawEvents, context).map((event) => ({
    type: event.type,
    payload: redactor.redactValue(event.payload),
  }));
  const projectedCalls = projectToolCalls(rawEvents, context).map((call) => ({
    name: call.name,
    input: redactor.redactValue(call.input),
    ok: call.ok,
    output: redactor.redactString(call.output),
  }));
  const files = await takeFileSnapshot(context.cwd);
  return { toolCalls: projectedCalls, files, events: projectedEvents };
}

/** 轨迹回放（spec §8.7）：按轨迹里的输入重跑一次，与已录的观察比较。 */
export async function replayTrajectory(
  trajectoryPath: string,
  options: { sessionFactory: SessionFactory; redactor: Redactor; defaultTimeoutMs?: number },
): Promise<CaseResult> {
  const trajectory = await readTrajectory(trajectoryPath);
  const meta = trajectory.meta;
  const evalCase: EvalCase = {
    case: meta.case,
    prompt: meta.prompt,
    seed: meta.seed,
    seedBytesHex: meta.seedBytesHex,
    generatedSeed: meta.generatedSeed,
    script: meta.script,
    followUpTurns: meta.followUpTurns,
    expect: {
      toolCalls: trajectory.toolCalls,
      files: trajectory.files,
      events: trajectory.events,
    },
  };
  const tmpRoot = await mkdtemp(join(tmpdir(), "mode-eval-replay-"));
  try {
    return await runSingleCase(evalCase, {
      sessionFactory: options.sessionFactory,
      redactor: options.redactor,
      timeoutMs: evalCase.timeoutMs ?? options.defaultTimeoutMs ?? 120_000,
      trajectoriesRoot: tmpRoot,
    });
  } finally {
    await rm(tmpRoot, { recursive: true, force: true }).catch(() => {});
  }
}

const MAX_MISMATCHES = 20;

/** 三层比较（spec §6）：工具调用 → 文件 → 事件；路径层面的差异点直接可读。 */
export function compareObserved(
  expected: { toolCalls: unknown; files: unknown; events: unknown },
  actual: { toolCalls: unknown; files: unknown; events: unknown },
  max = MAX_MISMATCHES,
): Mismatch[] {
  const mismatches: Mismatch[] = [];
  diffValues(expected, actual, "", mismatches, max);
  return mismatches.slice(0, max);
}

function diffValues(expected: unknown, actual: unknown, path: string, out: Mismatch[], max: number): void {
  if (out.length >= max) return;
  if (expected === actual) return;
  if (stableStringify(expected) === stableStringify(actual)) return;
  if (isPlainObject(expected) && isPlainObject(actual)) {
    const keys = new Set([...Object.keys(expected), ...Object.keys(actual)]);
    for (const key of [...keys].sort()) {
      diffValues(
        (expected as Record<string, unknown>)[key],
        (actual as Record<string, unknown>)[key],
        path ? `${path}.${key}` : key,
        out,
        max,
      );
    }
    return;
  }
  if (Array.isArray(expected) && Array.isArray(actual)) {
    if (expected.length !== actual.length) {
      out.push({ path: path || "$", expected: expected.length, actual: actual.length });
      return;
    }
    for (let index = 0; index < expected.length; index += 1) {
      diffValues(expected[index], actual[index], `${path}[${index}]`, out, max);
    }
    return;
  }
  out.push({ path: path || "$", expected, actual });
}

async function writeSeeds(cwd: string, evalCase: EvalCase): Promise<void> {
  for (const [name, content] of Object.entries(evalCase.seed ?? {})) {
    await mkdir(join(cwd, dirname(name)), { recursive: true });
    await writeFile(join(cwd, name), content, "utf8");
  }
  for (const [name, hex] of Object.entries(evalCase.seedBytesHex ?? {})) {
    await mkdir(join(cwd, dirname(name)), { recursive: true });
    await writeFile(join(cwd, name), Buffer.from(hex, "hex"));
  }
  for (const [name, spec] of Object.entries(evalCase.generatedSeed ?? {})) {
    await mkdir(join(cwd, dirname(name)), { recursive: true });
    const content = Array.from({ length: spec.lines }, () => spec.char.repeat(spec.width)).join("\n");
    await writeFile(join(cwd, name), content, "utf8");
  }
}

function substituteCwd(script: ScriptStep[], cwd: string): ScriptStep[] {
  const text = JSON.stringify(script).replaceAll("${cwd}", jsonEscape(cwd));
  return JSON.parse(text) as ScriptStep[];
}

function jsonEscape(value: string): string {
  return JSON.stringify(value).slice(1, -1);
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isTerminal(type: string): boolean {
  return type === "turn.completed" || type === "turn.failed";
}
