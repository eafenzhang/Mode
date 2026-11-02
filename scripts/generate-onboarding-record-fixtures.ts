/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明用例矩阵，拆分会降低可核对性。 */
/**
 * 生成 `onboarding-record-service` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/onboarding/onboardingRecordService.ts`
 * - `packages/shared/src/onboardingRecord.ts`
 *
 * 设备目录通过 `MODE_DATA_BASE_DIR` 隔离（记录落在 `<base>/.mode/v2/onboarding-record.json`），
 * userId / 已有任务由用例内可变状态注入；`decidedAt` 是 `new Date().toISOString()`，
 * 归一化为 `<ts>`。每条用例独立沙箱。
 *
 * ```bash
 * pnpm onboarding-record:fixtures
 * cargo test -p onboarding-record-service
 * ```
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import type { IOnboardingRecordService } from "../packages/services/src/onboarding/onboardingRecord.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "onboarding-record-service", "fixtures", "records.json");
const SANDBOX = mkdtempSync(path.join(tmpdir(), "mode-onboarding-fixtures-"));

// `paths.ts` 在模块加载时读取 MODE_DATA_BASE_DIR；必须在动态 import 之前设置。
process.env.MODE_DATA_BASE_DIR = SANDBOX;

interface Op {
  kind:
    | "setUser"
    | "setHasTask"
    | "append"
    | "claim"
    | "shouldOnboard"
    | "dismiss"
    | "latest"
    | "syncSettings"
    | "updatePrefs"
    | "getRecords"
    | "rawFile"
    | "clear"
    | "setupFile";
  userId?: string | null;
  hasTask?: boolean;
  deviceMid?: string;
  entry?: Record<string, unknown>;
  patch?: Record<string, unknown>;
  content?: string;
}

interface FixtureCase {
  name: string;
  ops: Op[];
  expected: unknown[];
}

const ENTRY = {
  occupation: "developer",
  interfaceMode: "coding",
  memoryEnabled: null,
  proactiveSuggestionsEnabled: true,
  completedAt: "2026-03-01T00:00:00.000Z",
};

function entry(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { ...ENTRY, ...over };
}

/** 与 TS `JSON.stringify(file, null, 2)` 同形的 seed 文件。 */
function recordFile(over: Record<string, unknown> = {}): string {
  return JSON.stringify(
    {
      version: 2,
      deviceMid: "dev-1",
      entries: [],
      decisions: [],
      ...over,
    },
    null,
    2,
  );
}

const ISO = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/gu;

function normalizeValue(value: unknown): unknown {
  if (typeof value === "string") return value.replace(ISO, "<ts>");
  if (Array.isArray(value)) return value.map(normalizeValue);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] = normalizeValue(child);
    }
    return out;
  }
  return value;
}

function normalizeError(error: unknown): unknown {
  return { error: error instanceof Error ? error.message : String(error) };
}

function recordPath(): string {
  return path.join(SANDBOX, ".mode", "v2", "onboarding-record.json");
}

function buildCases(): FixtureCase[] {
  return [
    {
      name: "append-overwrite",
      ops: [
        { kind: "setUser", userId: null },
        { kind: "append", deviceMid: "dev-1", entry: entry() },
        { kind: "getRecords" },
        { kind: "setUser", userId: "user-1" },
        { kind: "append", deviceMid: "dev-1", entry: entry({ occupation: "design" }) },
        { kind: "getRecords" },
        { kind: "setUser", userId: null },
        { kind: "append", deviceMid: "dev-1", entry: entry({ interfaceMode: "office" }) },
        { kind: "latest" },
        { kind: "rawFile" },
        { kind: "syncSettings" },
      ],
      expected: [],
    },
    {
      name: "claim-anonymous",
      ops: [
        {
          kind: "setupFile",
          content: recordFile({
            entries: [
              {
                userId: null,
                ...entry(),
                uploadState: "pending",
              },
            ],
          }),
        },
        { kind: "setUser", userId: "user-1" },
        { kind: "claim" },
        { kind: "getRecords" },
        { kind: "claim" },
        { kind: "getRecords" },
        {
          kind: "setupFile",
          content: recordFile({
            decisions: [
              {
                userId: null,
                status: "dismissed",
                reason: "user_closed",
                decidedAt: "2026-03-01T00:00:00.000Z",
              },
            ],
          }),
        },
        { kind: "setUser", userId: "user-2" },
        { kind: "claim" },
        { kind: "getRecords" },
        { kind: "setUser", userId: null },
        { kind: "claim" },
        { kind: "getRecords" },
      ],
      expected: [],
    },
    {
      name: "should-onboard",
      ops: [
        { kind: "setUser", userId: null },
        { kind: "setHasTask", hasTask: false },
        { kind: "shouldOnboard", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "setHasTask", hasTask: true },
        { kind: "shouldOnboard", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "shouldOnboard", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "setUser", userId: "user-9" },
        { kind: "shouldOnboard", deviceMid: "dev-1" },
        { kind: "getRecords" },
      ],
      expected: [],
    },
    {
      name: "dismiss",
      ops: [
        { kind: "setUser", userId: null },
        { kind: "dismiss", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "dismiss", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "setUser", userId: "user-1" },
        { kind: "dismiss", deviceMid: "dev-1" },
        { kind: "getRecords" },
        { kind: "append", deviceMid: "dev-1", entry: entry() },
        { kind: "dismiss", deviceMid: "dev-1" },
        { kind: "getRecords" },
      ],
      expected: [],
    },
    {
      name: "sync-settings",
      ops: [
        {
          kind: "setupFile",
          content: recordFile({
            entries: [
              { userId: null, ...entry({ occupation: "student" }), uploadState: "pending" },
              {
                userId: "user-1",
                ...entry({
                  occupation: "astronaut",
                  interfaceMode: "office",
                  memoryEnabled: false,
                  proactiveSuggestionsEnabled: null,
                }),
                uploadState: "pending",
              },
              {
                userId: null,
                ...entry({ occupation: "developer", memoryEnabled: null }),
                uploadState: "pending",
              },
            ],
          }),
        },
        { kind: "setUser", userId: null },
        { kind: "latest" },
        { kind: "syncSettings" },
        { kind: "setUser", userId: "user-1" },
        { kind: "syncSettings" },
        { kind: "setUser", userId: "user-2" },
        { kind: "syncSettings" },
        { kind: "latest" },
        { kind: "updatePrefs", patch: { memoryEnabled: true } },
        { kind: "getRecords" },
        { kind: "setUser", userId: null },
        {
          kind: "updatePrefs",
          patch: { memoryEnabled: false, proactiveSuggestionsEnabled: null },
        },
        { kind: "latest" },
        { kind: "syncSettings" },
      ],
      expected: [],
    },
    {
      name: "v1-upgrade",
      ops: [
        {
          kind: "setupFile",
          content: JSON.stringify(
            {
              version: 1,
              deviceMid: "dev-1",
              entries: [{ userId: null, ...entry(), uploadState: "pending" }],
            },
            null,
            2,
          ),
        },
        { kind: "getRecords" },
        { kind: "rawFile" },
        { kind: "setUser", userId: null },
        { kind: "append", deviceMid: "dev-1", entry: entry({ occupation: "student" }) },
        { kind: "getRecords" },
        { kind: "rawFile" },
      ],
      expected: [],
    },
    {
      name: "invalid-file",
      ops: [
        { kind: "setupFile", content: "not json" },
        { kind: "getRecords" },
        { kind: "setupFile", content: recordFile({ decisions: undefined }) },
        { kind: "getRecords" },
        {
          kind: "setupFile",
          content: recordFile({
            entries: [{ userId: null, ...entry(), uploadState: "uploaded" }],
          }),
        },
        { kind: "getRecords" },
        { kind: "setupFile", content: recordFile({ entries: [{ userId: "" }] }) },
        { kind: "getRecords" },
        { kind: "setUser", userId: null },
        { kind: "append", deviceMid: "dev-1", entry: entry() },
        { kind: "getRecords" },
        { kind: "clear" },
        { kind: "getRecords" },
        { kind: "clear" },
      ],
      expected: [],
    },
  ];
}

async function main(): Promise<void> {
  const { createOnboardingRecordService } = await import(
    "../packages/services/src/onboarding/onboardingRecordService.js"
  );
  const cases = buildCases();

  for (const testCase of cases) {
    // `paths.ts` 的 env 在模块加载时固化，所有用例共用同一记录路径；每个用例前清空。
    mkdirSync(path.join(SANDBOX, ".mode", "v2"), { recursive: true });
    rmSync(recordPath(), { force: true });
    let userId: string | null = null;
    let hasTask = false;
    const service: IOnboardingRecordService = createOnboardingRecordService({
      loadUserId: async () => userId,
      hasExistingLocalTask: async () => hasTask,
    });
    const expected: unknown[] = [];
    for (const op of testCase.ops) {
      try {
        switch (op.kind) {
          case "setUser":
            userId = op.userId ?? null;
            expected.push(null);
            break;
          case "setHasTask":
            hasTask = op.hasTask ?? false;
            expected.push(null);
            break;
          case "append":
            await service.appendRecord(op.deviceMid as string, op.entry as never);
            expected.push(null);
            break;
          case "claim":
            await service.claimAnonymousRecord();
            expected.push(null);
            break;
          case "shouldOnboard":
            expected.push(await service.shouldOnboard(op.deviceMid as string));
            break;
          case "dismiss":
            await service.dismissOnboarding(op.deviceMid as string);
            expected.push(null);
            break;
          case "latest":
            expected.push(await service.getLatestEntry());
            break;
          case "syncSettings":
            expected.push(await service.syncSettingsFromRecord());
            break;
          case "updatePrefs":
            await service.updateRecordPreferences(op.patch as never);
            expected.push(null);
            break;
          case "getRecords":
            expected.push(await service.getRecords());
            break;
          case "rawFile":
            expected.push(readFileSync(recordPath(), "utf-8"));
            break;
          case "clear":
            await service.clearRecords();
            expected.push(null);
            break;
          case "setupFile":
            writeFileSync(recordPath(), op.content as string);
            expected.push(null);
            break;
          default:
            throw new Error(`unknown op ${String((op as { kind?: string }).kind)}`);
        }
      } catch (error) {
        expected.push(normalizeError(error));
      }
    }
    testCase.expected = expected.map(normalizeValue);
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
