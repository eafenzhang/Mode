/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明取号/轮询/核销矩阵，拆分会降低可核对性。 */
/**
 * 生成 `off-peak-service` 远程路径（取号/Continue/批量轮询/核销/3102）的 TS 对拍数据。
 *
 * 真源：`packages/services/src/session/offPeakTaskService.ts`、`offPeakServerClient.ts`。
 *
 * ```bash
 * pnpm off-peak-service-remote:fixtures
 * cargo test -p off-peak-service
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ZodError } from "zod";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "off-peak-service",
  "fixtures",
  "off-peak-service-remote.json",
);
const NOW = 1_700_000_000_000;
const USABLE = { providerId: "provider-1", modelId: "m1" };

function normalize(value: unknown): unknown {
  if (typeof value === "string") {
    return value.startsWith("offpeak-") ? "<task-id>" : value;
  }
  if (Array.isArray(value)) return value.map(normalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, child]) => [
        key,
        normalize(child),
      ]),
    );
  }
  return value;
}

interface Seed {
  id: string;
  title: string;
  runState: "queued" | "running" | "completed" | "paused";
  serverTicketId?: string;
  boundSessionId?: string;
}

interface Case {
  name: string;
  seeds: Seed[];
  client: Record<string, unknown>;
  ops: Array<Record<string, unknown>>;
}

const CASES: Case[] = [
  {
    name: "create-queued",
    seeds: [],
    client: {
      take: [
        {
          ok: {
            ticketId: "tk-new",
            state: "queued",
            position: 2,
            nextPollAfterMs: 5000,
            registeredAt: NOW,
          },
        },
      ],
    },
    ops: [{ kind: "create" }, { kind: "list" }],
  },
  {
    name: "create-ready",
    seeds: [],
    client: {
      take: [{ ok: { ticketId: "tk-new", state: "ready", registeredAt: NOW } }],
    },
    ops: [{ kind: "create" }, { kind: "list" }],
  },
  {
    name: "create-errors",
    seeds: [],
    client: {
      take: [{ error: "3101" }, { error: "3103" }, { error: "network" }, { error: "invalid" }],
    },
    ops: [
      { kind: "create" },
      { kind: "create" },
      { kind: "create" },
      { kind: "create" },
      { kind: "list" },
    ],
  },
  {
    name: "create-validation",
    seeds: [],
    client: {},
    ops: [
      { kind: "create", params: { title: "  " } },
      { kind: "create", params: { modelSelection: { providerId: "provider-1", modelId: "bad" } } },
    ],
  },
  {
    name: "create-session-bound",
    seeds: [
      {
        id: "t1",
        title: "Bound",
        runState: "queued",
        serverTicketId: "tk-1",
        boundSessionId: "s1",
      },
    ],
    client: {},
    ops: [{ kind: "create", params: { boundSessionId: "s1" } }],
  },
  {
    name: "continue",
    seeds: [
      { id: "t1", title: "Alive", runState: "paused", serverTicketId: "tk-1" },
      { id: "t2", title: "Expired", runState: "paused", serverTicketId: "tk-2" },
      { id: "t3", title: "Unknown", runState: "paused", serverTicketId: "tk-3" },
    ],
    client: {
      batch: [
        { ok: { tickets: [{ ticketId: "tk-1", state: "ready", position: 1 }] } },
        { ok: { tickets: [{ ticketId: "tk-2", state: "expired" }] } },
        { error: "network" },
      ],
      take: [{ ok: { ticketId: "tk-4", state: "queued", registeredAt: NOW } }],
    },
    ops: [
      { kind: "continue", id: "t1" },
      { kind: "continue", id: "t2" },
      { kind: "continue", id: "t3" },
      { kind: "list" },
    ],
  },
  {
    name: "sync-and-settle",
    seeds: [
      { id: "t1", title: "Ready", runState: "queued", serverTicketId: "tk-1" },
      { id: "t2", title: "Expired", runState: "queued", serverTicketId: "tk-2" },
      { id: "t3", title: "Done ok", runState: "completed", serverTicketId: "tk-3" },
      { id: "t4", title: "Done 400", runState: "completed", serverTicketId: "tk-4" },
      { id: "t5", title: "Done network", runState: "completed", serverTicketId: "tk-5" },
    ],
    client: {
      batch: [
        {
          ok: {
            nextPollAfterMs: 1000,
            tickets: [
              { ticketId: "tk-1", state: "ready", position: 1 },
              { ticketId: "tk-2", state: "expired" },
            ],
          },
        },
      ],
      take: [{ ok: { ticketId: "tk-6", state: "queued", registeredAt: NOW } }],
      settle: { "tk-3": "ok", "tk-4": "400", "tk-5": "network" },
    },
    ops: [{ kind: "sync" }, { kind: "list" }],
  },
  {
    name: "handle-expired-and-support",
    seeds: [
      {
        id: "t1",
        title: "Running",
        runState: "running",
        serverTicketId: "tk-1",
      },
    ],
    client: {
      take: [{ ok: { ticketId: "tk-2", state: "queued", registeredAt: NOW } }],
      availability: { ok: { canTakeNumber: true, nextTakeAt: NOW + 10_000 } },
    },
    ops: [
      { kind: "handleExpired", id: "t1" },
      { kind: "list" },
      { kind: "availability" },
      { kind: "support" },
    ],
  },
];

async function main(): Promise<void> {
  const { OffPeakTaskService } = await import(
    "../packages/services/src/session/offPeakTaskService.js"
  );
  const { OffPeakTaskRepo } = await import("../packages/services/src/session/offPeakTaskRepo.js");
  const { OffPeakServerError } = await import(
    "../packages/services/src/session/offPeakServerClient.js"
  );
  const sandbox = mkdtempSync(path.join(tmpdir(), "mode-off-peak-remote-"));
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
            prompt: "p",
            permissionMode: "edit",
            modelSelection: USABLE as never,
            workspacePath: "/ws/demo",
            boundSessionId: seed.boundSessionId,
          },
          {
            now: NOW,
            offPeakTaskId: seed.id,
            serverTicketId: seed.serverTicketId,
            registeredAt: seed.serverTicketId ? NOW : undefined,
          },
        );
        if (seed.runState === "running") {
          await repo.markRunning(seed.id, { startedAt: NOW, conversationId: "conv-1" });
        } else if (seed.runState === "completed") {
          await repo.markTerminal(seed.id, { status: "completed", endedAt: NOW });
        } else if (seed.runState === "paused") {
          await repo.setPaused(seed.id, true, { now: NOW });
        }
      }
      const calls: unknown[] = [];
      const events: unknown[] = [];
      const take = [...((testCase.client.take as unknown[]) ?? [])];
      const batch = [...((testCase.client.batch as unknown[]) ?? [])];
      const settle = (testCase.client.settle as Record<string, string>) ?? {};
      const client = {
        async getTakeNumberAvailability() {
          calls.push({ kind: "availability" });
          const availability = testCase.client.availability as { ok: unknown } | undefined;
          if (!availability) throw new Error("no availability");
          return availability.ok as never;
        },
        async takeTicket(taskId: string) {
          calls.push({ kind: "take", taskId });
          const next = take.shift() as { ok?: unknown; error?: string } | undefined;
          if (!next) throw new Error("no take script");
          if (next.error === "3101") throw new OffPeakServerError("quota", 400, 3101);
          if (next.error === "3103") throw new OffPeakServerError("quota", 400, 3103);
          if (next.error === "invalid") throw new ZodError([]);
          if (next.error) throw new Error(next.error);
          return next.ok as never;
        },
        async batchStatus(ticketIds: string[]) {
          calls.push({ kind: "batch", ticketIds });
          const next = batch.shift() as { ok?: unknown; error?: string } | undefined;
          if (!next) throw new Error("no batch script");
          if (next.error) throw new Error(next.error);
          return next.ok as never;
        },
        async settle(ticketId: string) {
          calls.push({ kind: "settle", ticketId });
          const behavior = settle[ticketId];
          if (behavior === "400") throw new OffPeakServerError("gone", 400);
          if (behavior === "500") throw new OffPeakServerError("server", 500);
          if (behavior === "network") throw new Error("network");
        },
      };
      const service = new OffPeakTaskService({
        repo,
        client: client as never,
        resolveCodingPlanSupport: async () => ({ supported: true }) as never,
        resolveTelemetryProviderName: async () => "provider",
        resolveModelSelection: async (input: { modelId: string }) => {
          if (input.modelId.startsWith("bad")) return { ok: false, validation: {} } as never;
          return {
            ok: true,
            selection: { providerId: "provider-1", modelId: input.modelId },
          } as never;
        },
        logger: { warn() {}, info() {}, error() {}, debug() {} } as never,
        onTasksChanged: () => events.push({ kind: "changed" }),
        requestSchedulerWake: () => events.push({ kind: "wake" }),
        now: () => NOW,
      });
      const expected: unknown[] = [];
      for (const op of testCase.ops) {
        switch (op.kind) {
          case "create": {
            const params = {
              title: "T",
              prompt: "P",
              permissionMode: "edit",
              modelSelection: USABLE,
              workspacePath: "/ws/demo",
              ...(op.params as Record<string, unknown>),
            };
            expected.push(normalize(await service.createTask(params as never)));
            break;
          }
          case "continue":
            expected.push(normalize(await service.continueTask(op.id as string)));
            break;
          case "sync":
            await service.runSyncCycle();
            expected.push(null);
            break;
          case "handleExpired":
            await service.handleTicketExpiredDuringRun(op.id as string);
            expected.push(null);
            break;
          case "availability":
            expected.push(normalize(await service.getTakeNumberAvailability()));
            break;
          case "support":
            expected.push(normalize(await service.getCodingPlanSupport()));
            break;
          case "list":
            expected.push(normalize(await service.list()));
            break;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
      cases.push({
        name: testCase.name,
        seeds: testCase.seeds,
        client: testCase.client,
        ops: testCase.ops,
        expected,
        calls: normalize(calls),
        events: normalize(events),
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
