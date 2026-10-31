/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 broadcast 用例，拆分会降低可核对性。 */
/**
 * 生成 `broadcast-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/broadcast/broadcastService.ts`（`parentPort === null` 分支）
 *
 * ```bash
 * pnpm broadcast:fixtures
 * cargo test -p broadcast-service
 * ```
 */
import path from "node:path";
import { fileURLToPath } from "node:url";
import { mkdirSync, writeFileSync } from "node:fs";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "broadcast-service", "fixtures", "broadcast.json");

interface FixtureOp {
  kind: "send" | "acquire" | "commit" | "release" | "tryClaim";
  key?: string;
  message?: unknown;
}

interface FixtureCase {
  name: string;
  files: Array<{ path: string; content: string }>;
  ops: FixtureOp[];
  expected: unknown[];
  tree: Array<{ path: string; content: string }>;
}

interface BroadcastServiceLike {
  send(message: unknown): Promise<void>;
  acquireClaim(key: string): Promise<unknown>;
  commitClaim(lease: unknown): Promise<void>;
  releaseClaim(lease: unknown): Promise<void>;
  tryClaim(key: string): Promise<boolean>;
}

function normalizeValue(value: unknown, key?: string): unknown {
  if (typeof value === "string") {
    return key === "token" ? "<token>" : value;
  }
  if (typeof value === "number" && key === "retryAfterMs") {
    return "<retry>";
  }
  if (Array.isArray(value)) return value.map((item) => normalizeValue(item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value as Record<string, unknown>)) {
      out[childKey] = normalizeValue(child, childKey);
    }
    return out;
  }
  return value;
}

async function runCase(
  createService: () => BroadcastServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<FixtureCase> {
  const service = createService();
  const leases = new Map<string, unknown>();
  const expected: unknown[] = [];
  for (const op of testCase.ops) {
    expected.push(normalizeValue(await runOp(service, leases, op)));
  }
  return {
    name: testCase.name,
    files: testCase.files,
    ops: testCase.ops,
    expected,
    tree: [],
  };
}

async function runOp(
  service: BroadcastServiceLike,
  leases: Map<string, unknown>,
  op: FixtureOp,
): Promise<unknown> {
  try {
    switch (op.kind) {
      case "send":
        await service.send(op.message);
        return { ok: true };
      case "acquire": {
        const result = (await service.acquireClaim(op.key ?? "")) as {
          status: string;
          lease?: unknown;
        };
        if (result.status === "acquired" && result.lease) {
          leases.set(op.key ?? "", result.lease);
        }
        return result;
      }
      case "commit":
        await service.commitClaim(leases.get(op.key ?? ""));
        return { ok: true };
      case "release":
        await service.releaseClaim(leases.get(op.key ?? ""));
        return { ok: true };
      case "tryClaim":
        return await service.tryClaim(op.key ?? "");
      default:
        throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

async function main(): Promise<void> {
  const { createBroadcastService } =
    await import("../packages/services/src/broadcast/broadcastService.js");

  const cases: FixtureCase[] = [
    await runCase(() => createBroadcastService(null) as BroadcastServiceLike, {
      name: "claims",
      files: [],
      ops: [
        { kind: "acquire", key: "state:theme" },
        { kind: "acquire", key: "state:theme" },
        { kind: "commit", key: "state:theme" },
        { kind: "acquire", key: "state:theme" },
        { kind: "release", key: "state:theme" },
        { kind: "acquire", key: "   " },
        { kind: "tryClaim", key: "once" },
        { kind: "tryClaim", key: "once" },
        { kind: "acquire", key: " spaced " },
      ],
    }),
    await runCase(() => createBroadcastService(null) as BroadcastServiceLike, {
      name: "send",
      files: [],
      ops: [
        {
          kind: "send",
          message: {
            channel: "state:theme",
            payload: { theme: "dark" },
            sourceWindowId: 2,
          },
        },
        { kind: "send", message: { channel: "state:locale", payload: "zh-CN" } },
      ],
    }),
  ];

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
}

await main();
