/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明闲时任务本地生命周期矩阵，拆分会降低可核对性。 */
/**
 * 生成 `off-peak-service` 本地生命周期（list/get/pause/deleteHistory/cancel/delete/update/validate）
 * 的 Rust 对拍数据。取号/轮询/核销 outbox 不在本片。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/offPeakTaskService.ts`
 *
 * ```bash
 * pnpm off-peak-service:fixtures
 * cargo test -p off-peak-service
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "off-peak-service",
  "fixtures",
  "off-peak-service.json",
);
const NOW = 1_700_000_000_000;

function selection(modelId: string, reasoningLevel?: string): Record<string, unknown> {
  return {
    providerId: "provider-1",
    modelId,
    ...(reasoningLevel ? { options: { reasoningLevel } } : {}),
  };
}

const USABLE = selection("m1");
const USABLE_HIGH = selection("m1", "high");
const BAD = selection("bad-model", "low");

interface Seed {
  id: string;
  title: string;
  prompt: string;
  modelSelection: Record<string, unknown>;
  runState: "queued" | "running" | "completed" | "paused";
  conversationId?: string;
}

interface Op {
  kind: "pause" | "deleteHistory" | "cancel" | "delete" | "list" | "get" | "validate" | "update";
  id?: string;
  params?: Record<string, unknown>;
  selection?: Record<string, unknown>;
}

interface Case {
  name: string;
  seeds: Seed[];
  ops: Op[];
}

const CASES: Case[] = [
  {
    name: "list-projection",
    seeds: [
      { id: "t1", title: "One", prompt: "p1", modelSelection: USABLE, runState: "queued" },
      { id: "t2", title: "Two", prompt: "p2", modelSelection: BAD, runState: "queued" },
      { id: "t3", title: "Three", prompt: "p3", modelSelection: BAD, runState: "completed" },
      { id: "t4", title: "Four", prompt: "p4", modelSelection: USABLE_HIGH, runState: "paused" },
    ],
    ops: [{ kind: "list" }, { kind: "get", id: "t2" }, { kind: "get", id: "t4" }],
  },
  {
    name: "pause-and-history",
    seeds: [
      { id: "t1", title: "One", prompt: "p1", modelSelection: USABLE, runState: "queued" },
      {
        id: "t2",
        title: "Two",
        prompt: "p2",
        modelSelection: USABLE,
        runState: "running",
        conversationId: "conv-2",
      },
    ],
    ops: [
      { kind: "pause", id: "t1" },
      { kind: "pause", id: "t1" },
      { kind: "deleteHistory", id: "t2" },
      { kind: "get", id: "t1" },
      { kind: "list" },
    ],
  },
  {
    name: "cancel",
    seeds: [
      {
        id: "t1",
        title: "One",
        prompt: "p1",
        modelSelection: USABLE,
        runState: "running",
        conversationId: "conv-1",
      },
      { id: "t2", title: "Two", prompt: "p2", modelSelection: USABLE, runState: "completed" },
      { id: "t3", title: "Three", prompt: "p3", modelSelection: USABLE, runState: "queued" },
    ],
    ops: [
      { kind: "cancel", id: "t1" },
      { kind: "cancel", id: "t2" },
      { kind: "cancel", id: "missing" },
      { kind: "list" },
    ],
  },
  {
    name: "delete",
    seeds: [
      { id: "t1", title: "One", prompt: "p1", modelSelection: USABLE, runState: "queued" },
      { id: "t2", title: "Two", prompt: "p2", modelSelection: USABLE, runState: "completed" },
    ],
    ops: [
      { kind: "delete", id: "t1" },
      { kind: "delete", id: "t2" },
      { kind: "delete", id: "missing" },
      { kind: "list" },
    ],
  },
  {
    name: "update",
    seeds: [
      { id: "t1", title: "One", prompt: "p1", modelSelection: USABLE_HIGH, runState: "queued" },
      { id: "t2", title: "Two", prompt: "p2", modelSelection: USABLE, runState: "paused" },
      {
        id: "t3",
        title: "Three",
        prompt: "p3",
        modelSelection: USABLE,
        runState: "running",
        conversationId: "conv-3",
      },
      { id: "t4", title: "Four", prompt: "p4", modelSelection: USABLE, runState: "completed" },
    ],
    ops: [
      {
        kind: "update",
        id: "t1",
        params: {
          title: "New",
          prompt: "p2",
          permissionMode: "plan",
          modelSelection: selection("m2"),
        },
      },
      { kind: "update", id: "t2", params: { title: "Renamed" } },
      { kind: "update", id: "t3", params: { title: "X" } },
      { kind: "update", id: "t4", params: { title: "X" } },
      { kind: "update", id: "t1", params: { modelSelection: null } },
      { kind: "update", id: "t1", params: { modelSelection: BAD } },
      { kind: "update", id: "missing", params: { title: "X" } },
      { kind: "get", id: "t1" },
      { kind: "list" },
    ],
  },
  {
    name: "validate",
    seeds: [],
    ops: [
      { kind: "validate", selection: selection("m1") },
      { kind: "validate", selection: BAD },
      { kind: "validate", selection: { providerId: "provider-2", modelId: "m1" } },
    ],
  },
];

async function main(): Promise<void> {
  const { OffPeakTaskService } = await import(
    "../packages/services/src/session/offPeakTaskService.js"
  );
  const { OffPeakTaskRepo } = await import("../packages/services/src/session/offPeakTaskRepo.js");
  const sandbox = mkdtempSync(path.join(tmpdir(), "mode-off-peak-service-"));
  const realNow = Date.now;
  Date.now = () => NOW;
  const cases: unknown[] = [];
  try {
    for (const [index, testCase] of CASES.entries()) {
      const repo = new OffPeakTaskRepo(path.join(sandbox, `case-${index}.sqlite`));
      for (const seed of testCase.seeds) {
        await repo.create(
          {
            title: seed.title,
            prompt: seed.prompt,
            permissionMode: "edit",
            modelSelection: seed.modelSelection as never,
            workspacePath: "/ws/demo",
          },
          { now: NOW, offPeakTaskId: seed.id },
        );
        if (seed.runState === "running") {
          await repo.markRunning(seed.id, {
            startedAt: NOW,
            conversationId: seed.conversationId ?? "conv",
          });
        } else if (seed.runState === "completed") {
          await repo.markTerminal(seed.id, { status: "completed", endedAt: NOW });
        } else if (seed.runState === "paused") {
          await repo.setPaused(seed.id, true, { now: NOW });
        }
      }
      const events: unknown[] = [];
      const service = new OffPeakTaskService({
        repo,
        client: {} as never,
        resolveCodingPlanSupport: async () => ({}) as never,
        resolveTelemetryProviderName: async () => "provider",
        resolveModelSelection: async (input: { modelId: string; reasoningLevel?: string }) => {
          if (input.modelId.startsWith("bad")) {
            return { ok: false, validation: {} } as never;
          }
          return {
            ok: true,
            selection: selection(input.modelId, input.reasoningLevel),
          } as never;
        },
        logger: { warn() {}, info() {}, error() {}, debug() {} } as never,
        onTasksChanged: () => {
          events.push({ kind: "changed" });
        },
        stopRunningTask: async (params: Record<string, unknown>) => {
          events.push({ kind: "stop", ...params });
        },
        now: () => NOW,
      });
      const expected: unknown[] = [];
      for (const op of testCase.ops) {
        let result: unknown;
        switch (op.kind) {
          case "list":
            await new Promise((resolve) => setTimeout(resolve, 0));
            result = await service.list();
            break;
          case "get":
            await new Promise((resolve) => setTimeout(resolve, 0));
            result = await service.get(op.id as string);
            break;
          case "pause":
            result = await service.pauseTask(op.id as string);
            break;
          case "deleteHistory":
            result = await service.deleteHistory(op.id as string);
            break;
          case "cancel":
            result = await service.cancelTask(op.id as string);
            await new Promise((resolve) => setTimeout(resolve, 0));
            break;
          case "delete":
            await service.deleteTask(op.id as string);
            result = null;
            await new Promise((resolve) => setTimeout(resolve, 0));
            break;
          case "update":
            result = await service.updateTask(op.id as string, op.params as never);
            break;
          case "validate":
            result = await service.validateDispatchModelSelection(op.selection as never);
            break;
        }
        expected.push(JSON.parse(JSON.stringify(result)) as unknown);
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
      cases.push({
        name: testCase.name,
        seeds: testCase.seeds,
        ops: testCase.ops,
        expected,
        events,
      });
      repo.close();
    }
  } finally {
    Date.now = realNow;
    rmSync(sandbox, { recursive: true, force: true });
  }
  const fixture = { now: NOW, cases };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${CASES.length} cases)\n`);
}

await main();
