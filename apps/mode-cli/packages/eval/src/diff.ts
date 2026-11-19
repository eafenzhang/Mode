// 任务级 diff（spec §3/§8）：对比两次 run 的产物目录，逐 case 定位到 turn/tool。
// 只比较行为三层（工具调用/文件/事件，剥掉 turn 元数据）；status 变化单独报告。
import { readdir } from "node:fs/promises";
import { join } from "node:path";

import { readTrajectory, type Trajectory } from "./trajectory.js";
import type { Mismatch } from "./types.js";

export type CaseDiff = {
  case: string;
  statusA?: string;
  statusB?: string;
  /** 该 case 只存在于其中一侧。 */
  onlyIn?: "A" | "B";
  /** 结构差异（不含 status；A 为 expected、B 为 actual）。 */
  mismatches: Array<Mismatch & { location?: string }>;
};

export type RunDiff = {
  schemaVersion: 1;
  dirA: string;
  dirB: string;
  identical: boolean;
  cases: CaseDiff[];
};

export function formatRunDiffText(diff: RunDiff): string {
  const lines: string[] = [];
  if (diff.identical) {
    lines.push(`eval diff: identical (${diff.cases.length} case(s))`);
    return `${lines.join("\n")}\n`;
  }
  lines.push(`eval diff: ${diff.dirA} vs ${diff.dirB}`);
  for (const item of diff.cases) {
    if (item.onlyIn !== undefined) {
      lines.push(`  [only-in-${item.onlyIn}] ${item.case} (statusA=${item.statusA ?? "-"}, statusB=${item.statusB ?? "-"})`);
      continue;
    }
    const statusChanged = item.statusA !== item.statusB;
    lines.push(
      `  [${statusChanged ? `status ${item.statusA}→${item.statusB}` : "diff"}] ${item.case}: ${item.mismatches.length} mismatch(es)`,
    );
    for (const mismatch of item.mismatches.slice(0, 5)) {
      const at = mismatch.location ? ` @${mismatch.location}` : "";
      lines.push(`    - ${mismatch.path}${at}: expected ${JSON.stringify(mismatch.expected)}, got ${JSON.stringify(mismatch.actual)}`);
    }
    if (item.mismatches.length > 5) {
      lines.push(`    - … ${item.mismatches.length - 5} more mismatch(es)`);
    }
  }
  return `${lines.join("\n")}\n`;
}

const MAX_MISMATCHES = 20;

export async function diffRuns(dirA: string, dirB: string): Promise<RunDiff> {
  const casesA = await listTrajectoryCases(dirA);
  const casesB = await listTrajectoryCases(dirB);
  const ids = [...new Set([...casesA, ...casesB])].sort();
  const cases: CaseDiff[] = [];
  for (const id of ids) {
    const inA = casesA.includes(id);
    const inB = casesB.includes(id);
    if (!inA || !inB) {
      cases.push({
        case: id,
        onlyIn: inA ? "A" : "B",
        statusA: inA ? await readStatus(join(dirA, "trajectories", `${id}.jsonl`)) : undefined,
        statusB: inB ? await readStatus(join(dirB, "trajectories", `${id}.jsonl`)) : undefined,
        mismatches: [],
      });
      continue;
    }
    const trajectoryA = await readTrajectory(join(dirA, "trajectories", `${id}.jsonl`));
    const trajectoryB = await readTrajectory(join(dirB, "trajectories", `${id}.jsonl`));
    const mismatches = diffTrajectories(trajectoryA, trajectoryB);
    cases.push({
      case: id,
      statusA: trajectoryA.result?.status,
      statusB: trajectoryB.result?.status,
      mismatches,
    });
  }
  const identical = cases.every(
    (item) =>
      item.onlyIn === undefined &&
      item.mismatches.length === 0 &&
      item.statusA === item.statusB,
  );
  return { schemaVersion: 1, dirA, dirB, identical, cases };
}

/** 三层结构差异 + 差异点定位（事件 → turn，工具 → tool 名与序号）。 */
export function diffTrajectories(trajectoryA: Trajectory, trajectoryB: Trajectory): Array<Mismatch & { location?: string }> {
  const found: Mismatch[] = [];
  diffValues(
    {
      toolCalls: stripToolTurns(trajectoryA.toolCalls),
      files: trajectoryA.files,
      events: stripEventTurns(trajectoryA.events),
    },
    {
      toolCalls: stripToolTurns(trajectoryB.toolCalls),
      files: trajectoryB.files,
      events: stripEventTurns(trajectoryB.events),
    },
    "",
    found,
  );
  return found.slice(0, MAX_MISMATCHES).map((mismatch) => ({
    ...mismatch,
    location: locate(mismatch.path, trajectoryA, trajectoryB),
  }));
}

/** `events[3].payload.x` → `turn=<n>`；`toolCalls[1].output` → `tool=<name>#1`。 */
function locate(path: string, a: Trajectory, b: Trajectory): string | undefined {
  const eventMatch = path.match(/^events\[(\d+)\]/);
  if (eventMatch) {
    const index = Number(eventMatch[1]);
    const turn = a.events[index]?.turn ?? b.events[index]?.turn;
    return turn === undefined ? undefined : `turn=${turn}`;
  }
  const toolMatch = path.match(/^toolCalls\[(\d+)\]/);
  if (toolMatch) {
    const index = Number(toolMatch[1]);
    const name = a.toolCalls[index]?.name ?? b.toolCalls[index]?.name ?? "<unknown>";
    return `tool=${name}#${index}`;
  }
  return undefined;
}

function stripEventTurns(events: Trajectory["events"]): Array<{ type: string; payload: unknown }> {
  return events.map(({ turn: _turn, ...rest }) => rest);
}

function stripToolTurns(calls: Trajectory["toolCalls"]): Array<{ name: string; input: unknown; ok: boolean; output: string }> {
  return calls.map(({ turn: _turn, ...rest }) => rest);
}

async function readStatus(path: string): Promise<string | undefined> {
  try {
    const trajectory = await readTrajectory(path);
    return trajectory.result?.status;
  } catch {
    return undefined;
  }
}

async function listTrajectoryCases(dir: string): Promise<string[]> {
  let entries: string[];
  try {
    entries = await readdir(join(dir, "trajectories"));
  } catch {
    return [];
  }
  return entries.filter((entry) => entry.endsWith(".jsonl")).map((entry) => entry.replace(/\.jsonl$/, ""));
}

/** 路径级差异（与 runner 的 compareObserved 同规则；此处独立实现以携带路径信息）。 */
function diffValues(expected: unknown, actual: unknown, path: string, out: Array<Mismatch & { location?: string }>): void {
  if (out.length >= MAX_MISMATCHES) return;
  if (expected === actual) return;
  if (JSON.stringify(sortValue(expected)) === JSON.stringify(sortValue(actual))) return;
  if (isPlainObject(expected) && isPlainObject(actual)) {
    const keys = new Set([...Object.keys(expected), ...Object.keys(actual)]);
    for (const key of [...keys].sort()) {
      diffValues(
        (expected as Record<string, unknown>)[key],
        (actual as Record<string, unknown>)[key],
        path ? `${path}.${key}` : key,
        out,
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
      diffValues(expected[index], actual[index], `${path}[${index}]`, out);
    }
    return;
  }
  out.push({ path: path || "$", expected, actual });
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
        .map(([key, inner]) => [key, sortValue(inner)]),
    );
  }
  return value;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
