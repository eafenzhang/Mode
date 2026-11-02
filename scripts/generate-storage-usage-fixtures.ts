/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明存储分类/清理/占用矩阵，拆分会降低可核对性。 */
/**
 * 生成 `storage-usage` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/storage/domain/storageCatalog.ts`
 * - `packages/services/src/storage/domain/usageAggregate.ts`
 * - `packages/shared/src/storage.ts`
 *
 * ```bash
 * pnpm storage-usage:fixtures
 * cargo test -p storage-usage
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  STORAGE_CATEGORY_IDS,
  groupStorageRootsByVolume,
  type StorageRootSpec,
  type StorageRootUsage,
} from "../packages/shared/src/storage.js";
import {
  classifyStoragePath,
  getStorageCategoryCleanability,
  getStorageCleanScopes,
  isProtectedStoragePath,
} from "../packages/services/src/storage/domain/storageCatalog.js";
import {
  planStorageClean,
  type StorageCleanCandidate,
} from "../packages/services/src/storage/domain/cleanPlan.js";
import {
  createStorageUsageAccumulator,
  type StorageScanEntry,
} from "../packages/services/src/storage/domain/usageAggregate.js";
import { createScanJob } from "../packages/services/src/storage/app/scanJob.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "storage-usage", "fixtures", "storage-usage.json");

const context = (rootId: "home" | "dataBaseDir", hasCustomDataBaseDir = false) => ({
  rootId,
  hasCustomDataBaseDir,
});

const classificationPaths: Array<{
  rawPath: string;
  context: ReturnType<typeof context>;
}> = [
  { rawPath: "", context: context("home") },
  { rawPath: "/", context: context("home") },
  { rawPath: "\\v2\\logs\\a.log", context: context("home") },
  { rawPath: "v2/sessions", context: context("home") },
  { rawPath: "v2/sessions/abc/session.json", context: context("home") },
  { rawPath: "v2/session-bindings/x", context: context("home") },
  { rawPath: "v2/checkpoints/abc/pending/x", context: context("home") },
  { rawPath: "v2/checkpoints/abc/tmp/y", context: context("home") },
  { rawPath: "v2/checkpoints/abc/committed/z", context: context("home") },
  { rawPath: "cli/agents/a/b/transcript.jsonl", context: context("home") },
  { rawPath: "cli/agents/a/b/other.jsonl", context: context("home") },
  { rawPath: "cli/agents/a/b/transcript.jsonl.bak", context: context("home") },
  { rawPath: "v2/session.sqlite", context: context("home") },
  { rawPath: "v2/session.sqlite-wal", context: context("home") },
  { rawPath: "v2/session.sqlite-shm", context: context("home") },
  { rawPath: "cli/db/db.sqlite", context: context("home") },
  { rawPath: "cli/db/db.sqlite-wal", context: context("home") },
  { rawPath: "cli/db/db.sqlite.backup", context: context("home") },
  { rawPath: "cli/config.json.bak", context: context("home") },
  { rawPath: "cli/config.json.bak.1", context: context("home") },
  { rawPath: "v2/x.bak", context: context("home") },
  { rawPath: "v2/x.backup.json", context: context("home") },
  { rawPath: "v2/setting.json.corrupt-1", context: context("home") },
  { rawPath: "v2/setting.json.foo-backup-bar", context: context("home") },
  { rawPath: "v2/config.json.pre-abc", context: context("home") },
  { rawPath: "v2/coding-plan-cache.json", context: context("home") },
  { rawPath: "v2/bots-model-cache.json", context: context("home") },
  { rawPath: "v2/bots-model-cache-v2.json", context: context("home") },
  { rawPath: "computer-use/run/x.log", context: context("home") },
  { rawPath: "computer-use/run/x.txt", context: context("home") },
  { rawPath: "v2/foo.json", context: context("home") },
  { rawPath: "v2/config.json", context: context("home") },
  { rawPath: "cli/config.json", context: context("home") },
  { rawPath: "agents/x.md", context: context("home") },
  { rawPath: "AGENTS.md", context: context("home") },
  { rawPath: "agent/foo", context: context("home") },
  { rawPath: "agentic", context: context("home") },
  { rawPath: "backup/x", context: context("home") },
  { rawPath: "v2/backup/x", context: context("home") },
  { rawPath: "v2/migrations/x", context: context("home") },
  { rawPath: "cli/db/backup/x", context: context("home") },
  { rawPath: "cli/db/backups/x", context: context("home") },
  { rawPath: "export-log/x", context: context("home") },
  { rawPath: "export-log-stage/x", context: context("home") },
  { rawPath: "logs/x", context: context("home") },
  { rawPath: "v2/logs/x", context: context("home") },
  { rawPath: "cli/log/x", context: context("home") },
  { rawPath: "v2/crash/x", context: context("home") },
  { rawPath: "v2/crash/live/core.bin", context: context("home") },
  { rawPath: "v2/perf/x", context: context("home") },
  { rawPath: "cli/debug/x", context: context("home") },
  { rawPath: "cli/rollout/x", context: context("home") },
  { rawPath: "v2/dev/x", context: context("home") },
  { rawPath: "v2/acp-traffic-proxy/x", context: context("home") },
  { rawPath: "v2/acp-stream-diagnostics/x", context: context("home") },
  { rawPath: "cli/artifacts/x", context: context("home") },
  { rawPath: "clipboard/x", context: context("home") },
  { rawPath: "git-checkpoint-index/x", context: context("home") },
  { rawPath: "editor-icon/x", context: context("home") },
  { rawPath: "tmp/x", context: context("home") },
  { rawPath: "cache/x", context: context("home") },
  { rawPath: "agents/x", context: context("home") },
  { rawPath: "bundled-agents/x", context: context("home") },
  { rawPath: "lite/x", context: context("home") },
  { rawPath: "computer-use/x", context: context("home") },
  { rawPath: "cli/plugins/x", context: context("home") },
  { rawPath: "cli/plugins/cache/x", context: context("home") },
  { rawPath: "v2/agent-config/x", context: context("home") },
  { rawPath: "v2/bots-runtime-locks/x", context: context("home") },
  { rawPath: "v2/bot-attachments/x", context: context("home") },
  { rawPath: "v2/certs/x", context: context("home") },
  { rawPath: "v2/acp-auth/x", context: context("home") },
  { rawPath: "v2/acp-config/x", context: context("home") },
  { rawPath: "v2/provider/x", context: context("home") },
  { rawPath: "cli/models/x", context: context("home") },
  { rawPath: "cli/memories/x", context: context("home") },
  { rawPath: "cli/workflows/x", context: context("home") },
  { rawPath: "security/x", context: context("home") },
  { rawPath: "commands/x", context: context("home") },
  { rawPath: "skills/x", context: context("home") },
  { rawPath: "workflows/x", context: context("home") },
  { rawPath: "workspace/x", context: context("home") },
  { rawPath: "mailbox/x", context: context("home") },
  { rawPath: "server/x", context: context("home") },
  { rawPath: "controller/x", context: context("home") },
  { rawPath: "launcher/x", context: context("home") },
  { rawPath: "dev-signing/x", context: context("home") },
  { rawPath: "cua-helper-dev-identity/x", context: context("home") },
  { rawPath: "perf-task-manifests/x", context: context("home") },
  { rawPath: "plugin-workspace/x", context: context("home") },
  { rawPath: "projects/x", context: context("home") },
  { rawPath: "unknown/x", context: context("home") },
  { rawPath: "v2/unknown/x", context: context("home") },
  { rawPath: "v2/sessions/x", context: context("home", true) },
  { rawPath: "v2/provider/x", context: context("home", true) },
  { rawPath: "v2/provider/x", context: context("dataBaseDir", true) },
  { rawPath: "v2/certs/x", context: context("dataBaseDir") },
];

const protectedPaths = [
  "setting.json",
  "nested/setting.json",
  "setting.json.lock",
  "credentials.json",
  "nested/.credentials.json",
  ".tokens",
  "mode-stdio-tap.json",
  "v2/crash/live",
  "v2/crash/live/core.bin",
  "v2/crash/live2/core.bin",
  "v2\\crash\\live\\core.bin",
  "v2/setting.json",
  "v2/credentials.json.bak",
  "",
];

function scanEntry(relativePath: string, bytes: number, mtimeMs = 0): StorageScanEntry {
  return { relativePath, bytes, mtimeMs };
}

function snapshotFixture(options: {
  spec: StorageRootSpec;
  maxEntriesPerCategory?: number;
  volume: StorageRootUsage["volume"];
  entries: StorageScanEntry[];
}): Record<string, unknown> {
  const accumulator =
    options.maxEntriesPerCategory === undefined
      ? createStorageUsageAccumulator(options.spec)
      : createStorageUsageAccumulator(options.spec, {
          maxEntriesPerCategory: options.maxEntriesPerCategory,
        });
  for (const entry of options.entries) accumulator.add(entry);
  return {
    spec: options.spec,
    maxEntriesPerCategory: options.maxEntriesPerCategory ?? null,
    volume: options.volume,
    entries: options.entries,
    expected: accumulator.snapshot(options.volume),
  };
}

const homeSpec: StorageRootSpec = {
  id: "home",
  path: "/home/user/.mode",
  hasCustomDataBaseDir: false,
};

const volume = {
  deviceId: "dev-1",
  mountPoint: "/",
  totalBytes: 1_000_000,
  freeBytes: 250_000,
};

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 0, 2, 12, 0, 0);

function cleanCandidate(
  relativePath: string,
  mtimeMs: number,
  bytes = 1,
): StorageCleanCandidate {
  return { relativePath, bytes, mtimeMs };
}

function cleanPlanCase(options: {
  name: string;
  categoryId: Parameters<typeof planStorageClean>[0]["categoryId"];
  candidates: StorageCleanCandidate[];
  context: ReturnType<typeof context>;
  now: number;
  offsetSecs: number;
  timeZone: string;
}): Record<string, unknown> {
  const previousTimeZone = process.env.TZ;
  process.env.TZ = options.timeZone;
  try {
    return {
      name: options.name,
      categoryId: options.categoryId,
      candidates: options.candidates,
      context: options.context,
      now: options.now,
      offsetSecs: options.offsetSecs,
      timeZone: options.timeZone,
      expected: planStorageClean({
        categoryId: options.categoryId,
        candidates: options.candidates,
        context: options.context,
        now: options.now,
      }),
    };
  } finally {
    if (previousTimeZone === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = previousTimeZone;
    }
  }
}

function scanRoot(bytes: number, fileCount = 1): StorageRootUsage {
  return {
    id: "home",
    path: "/home/u/.mode",
    volume: null,
    bytes,
    fileCount,
    categories: [],
  };
}

type ScanStep =
  | { type: "progress"; now: number; bytes: number; errors?: Array<{ path: string; code: string }> }
  | { type: "timer"; now: number }
  | { type: "finish"; now: number; bytes: number; errors?: Array<{ path: string; code: string }> }
  | { type: "fail"; now: number; code: string; abort?: boolean }
  | { type: "cancel"; now: number };

async function scanJobCase(options: {
  name: string;
  startNow: number;
  throttleMs: number;
  steps: ScanStep[];
}): Promise<Record<string, unknown>> {
  let fakeNow = options.startNow;
  const emissions: unknown[] = [];
  const timers: Array<{ callback: () => void; delayMs: number; cancelled: boolean; fired: boolean }> = [];
  let control: {
    onProgress?: (progress: { roots: StorageRootUsage[]; errors: Array<{ path: string; code: string }> }) => void;
    resolve?: (progress: { roots: StorageRootUsage[]; errors: Array<{ path: string; code: string }> }) => void;
    reject?: (error: unknown) => void;
  } = {};
  const runner = {
    run(request: {
      signal: AbortSignal;
      onProgress: (progress: { roots: StorageRootUsage[]; errors: Array<{ path: string; code: string }> }) => void;
    }) {
      control.onProgress = request.onProgress;
      request.signal.addEventListener("abort", () => {
        control.reject?.(new DOMException("storage scan aborted", "AbortError"));
      });
      return new Promise<{ roots: StorageRootUsage[]; errors: Array<{ path: string; code: string }> }>(
        (resolve, reject) => {
          control.resolve = resolve;
          control.reject = reject;
        },
      );
    },
  };
  const roots = [
    { id: "home" as const, path: "/home/u/.mode", hasCustomDataBaseDir: false },
  ];
  const job = createScanJob({
    jobId: "scan-fixture",
    roots,
    runner,
    now: () => fakeNow,
    throttleMs: options.throttleMs,
    emit: (snapshot) => emissions.push(snapshot),
    setTimer: (callback, delayMs) => {
      const timer = { callback, delayMs, cancelled: false, fired: false };
      timers.push(timer);
      return timer;
    },
    clearTimer: (handle) => {
      (handle as { cancelled: boolean }).cancelled = true;
    },
  });
  for (const step of options.steps) {
    fakeNow = step.now;
    if (step.type === "progress") {
      control.onProgress?.({ roots: [scanRoot(step.bytes)], errors: step.errors ?? [] });
      continue;
    }
    if (step.type === "timer") {
      const timer = timers.find((candidate) => !candidate.fired && !candidate.cancelled);
      if (timer) {
        timer.fired = true;
        timer.callback();
      }
      continue;
    }
    if (step.type === "finish") {
      control.resolve?.({ roots: [scanRoot(step.bytes)], errors: step.errors ?? [] });
      break;
    }
    if (step.type === "fail") {
      const error = step.abort
        ? new DOMException("storage scan aborted", "AbortError")
        : Object.assign(new Error("scan failed"), { code: step.code });
      control.reject?.(error);
      break;
    }
    if (step.type === "cancel") {
      job.cancel();
      break;
    }
  }
  const final = await job.done;
  return {
    name: options.name,
    roots,
    startNow: options.startNow,
    throttleMs: options.throttleMs,
    steps: options.steps.map((step) =>
      step.type === "progress" || step.type === "finish"
        ? {
            type: step.type,
            now: step.now,
            roots: [scanRoot(step.bytes)],
            errors: step.errors ?? [],
          }
        : step,
    ),
    expected: {
      emissions,
      final,
      timerDelays: timers.map((timer) => timer.delayMs),
      timerCount: timers.length,
    },
  };
}

const fixture = {
  categoryIds: STORAGE_CATEGORY_IDS,
  cleanability: Object.fromEntries(
    STORAGE_CATEGORY_IDS.map((id) => [id, getStorageCategoryCleanability(id)]),
  ),
  cleanScopes: Object.fromEntries(
    STORAGE_CATEGORY_IDS.map((id) => [id, getStorageCleanScopes(id)]),
  ),
  classifications: classificationPaths.map(({ rawPath, context: catalogContext }) => ({
    rawPath,
    context: catalogContext,
    expected: classifyStoragePath(rawPath, catalogContext),
  })),
  protected: protectedPaths.map((rawPath) => ({
    rawPath,
    expected: isProtectedStoragePath(rawPath),
  })),
  snapshots: [
    snapshotFixture({
      spec: homeSpec,
      maxEntriesPerCategory: 2,
      volume,
      entries: [
        scanEntry("v2/sessions/a/s1.json", 10),
        scanEntry("v2/sessions/a/s2.json", 5),
        scanEntry("v2/session-bindings/a/b", 3),
        scanEntry("v2/session-bindings/a/c", 4),
        scanEntry("v2/session-bindings/b/d", 1),
        scanEntry("v2/checkpoints/x/committed/y", 7),
        scanEntry("v2/logs/a.log", 3),
        scanEntry("v2/logs/b.log", 3),
        scanEntry("v2/logs/c.log", 1),
        scanEntry("v2/logs/d.log", 2),
        scanEntry("cache/a", 8),
        scanEntry("tmp/b", 9),
        scanEntry("cli/agents/a/b/transcript.jsonl", 20),
        scanEntry("cli/agents/a/c/transcript.jsonl", 15),
        scanEntry("v2/other/data", 6),
        scanEntry("unknown/x", 2),
        scanEntry("v2/sessions", 1),
      ],
    }),
    snapshotFixture({
      spec: { ...homeSpec, id: "dataBaseDir", path: "D:/ModeData/.mode", hasCustomDataBaseDir: false },
      maxEntriesPerCategory: 0,
      volume: null,
      entries: [scanEntry("v2/logs/a.log", 3), scanEntry("backup/x", 4)],
    }),
    snapshotFixture({
      spec: { ...homeSpec, hasCustomDataBaseDir: true },
      volume: null,
      entries: [scanEntry("v2/sessions/x", 5), scanEntry("v2/certs/x", 6)],
    }),
  ],
  cleanPlans: [
    cleanPlanCase({
      name: "non-cleanable-category",
      categoryId: "config",
      candidates: [cleanCandidate("v2/config.json", NOW), cleanCandidate("v2/certs/x", NOW)],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "logs-filter-same-day-and-scope",
      categoryId: "logs",
      candidates: [
        cleanCandidate("logs/a.log", Date.UTC(2026, 0, 2, 0, 0, 0)),
        cleanCandidate("logs/b.log", Date.UTC(2026, 0, 2, 11, 59, 59)),
        cleanCandidate("logs/c.log", Date.UTC(2026, 0, 1, 23, 59, 59)),
        cleanCandidate("cli/plugins/cache/x", Date.UTC(2025, 11, 31)),
        cleanCandidate("v2/logs/setting.json", Date.UTC(2025, 11, 31)),
      ],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "logs-local-day-positive-offset",
      categoryId: "logs",
      candidates: [
        cleanCandidate("logs/a.log", Date.UTC(2026, 0, 1, 15, 30, 0)),
        cleanCandidate("logs/b.log", Date.UTC(2026, 0, 1, 16, 30, 0)),
        cleanCandidate("logs/c.log", Date.UTC(2026, 0, 1, 16, 0, 0)),
      ],
      context: context("home"),
      now: Date.UTC(2026, 0, 2, 0, 30, 0),
      offsetSecs: 8 * 60 * 60,
      timeZone: "Asia/Shanghai",
    }),
    cleanPlanCase({
      name: "logs-local-day-negative-offset",
      categoryId: "logs",
      candidates: [
        cleanCandidate("logs/a.log", Date.UTC(2026, 0, 2, 4, 30, 0)),
        cleanCandidate("logs/b.log", Date.UTC(2026, 0, 2, 5, 30, 0)),
      ],
      context: context("home"),
      now: Date.UTC(2026, 0, 2, 3, 0, 0),
      offsetSecs: -5 * 60 * 60,
      timeZone: "Etc/GMT+5",
    }),
    cleanPlanCase({
      name: "subagent-activity-uses-all-candidates",
      categoryId: "subagentTranscripts",
      candidates: [
        cleanCandidate("cli/agents/s1/x/transcript.jsonl", NOW - 3 * DAY),
        cleanCandidate("cli/agents/s1/x/metadata.json", NOW),
        cleanCandidate("cli/agents/s2/x/transcript.jsonl", NOW - 25 * 60 * 60 * 1000),
        cleanCandidate("cli/agents/s2/x/output.json", NOW - 30 * 60 * 60 * 1000),
        cleanCandidate("cli/agents/s3/x/transcript.jsonl", NOW - DAY),
        cleanCandidate("logs/a.log", NOW - 3 * DAY),
      ],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "subagent-all-inactive",
      categoryId: "subagentTranscripts",
      candidates: [
        cleanCandidate("cli/agents/s1/x/transcript.jsonl", NOW - 2 * DAY),
        cleanCandidate("cli/agents/s1/x/metadata.json", NOW - 2 * DAY),
        cleanCandidate("cli/agents/s2/x/transcript.jsonl", NOW - DAY - 1),
      ],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "empty-candidates",
      categoryId: "backups",
      candidates: [],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "backups-filter-other-categories-and-protected",
      categoryId: "backups",
      candidates: [
        cleanCandidate("cli/db/db.sqlite.backup", NOW - 10 * DAY),
        cleanCandidate("cli/db/db.sqlite", NOW - 10 * DAY),
        cleanCandidate("v2/config.json.pre-x", NOW - 10 * DAY),
        cleanCandidate("v2/config.json", NOW - 10 * DAY),
        cleanCandidate("v2/crash/live/x", NOW - 10 * DAY),
      ],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "exports-scope",
      categoryId: "exports",
      candidates: [
        cleanCandidate("export-log/a", NOW - 10 * DAY),
        cleanCandidate("logs/a.log", NOW - 10 * DAY),
        cleanCandidate("export-log-stage/b", NOW - 10 * DAY),
      ],
      context: context("home"),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
    cleanPlanCase({
      name: "custom-data-root-moves-v2-to-other",
      categoryId: "logs",
      candidates: [cleanCandidate("v2/logs/a.log", NOW - 10 * DAY)],
      context: context("home", true),
      now: NOW,
      offsetSecs: 0,
      timeZone: "UTC",
    }),
  ],
  scanJobs: [
    await scanJobCase({
      name: "throttle-basic",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        { type: "progress", now: 1000, bytes: 1 },
        { type: "progress", now: 1100, bytes: 2 },
        { type: "progress", now: 1200, bytes: 3 },
        { type: "timer", now: 1300 },
        { type: "finish", now: 1400, bytes: 4 },
      ],
    }),
    await scanJobCase({
      name: "throttle-second-window",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        { type: "progress", now: 1000, bytes: 1 },
        { type: "progress", now: 1050, bytes: 2 },
        { type: "timer", now: 1300 },
        { type: "progress", now: 1400, bytes: 3 },
        { type: "progress", now: 1450, bytes: 4 },
        { type: "timer", now: 1600 },
        { type: "finish", now: 1700, bytes: 5 },
      ],
    }),
    await scanJobCase({
      name: "finish-without-progress",
      startNow: 2000,
      throttleMs: 300,
      steps: [{ type: "finish", now: 2100, bytes: 7 }],
    }),
    await scanJobCase({
      name: "failure-code",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        { type: "progress", now: 1000, bytes: 1 },
        { type: "fail", now: 1200, code: "EACCES" },
      ],
    }),
    await scanJobCase({
      name: "cancel-preserves-latest",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        {
          type: "progress",
          now: 1000,
          bytes: 1,
          errors: [{ path: "a", code: "EIO" }],
        },
        { type: "cancel", now: 1100 },
      ],
    }),
    await scanJobCase({
      name: "abort-error-without-cancel",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        {
          type: "progress",
          now: 1000,
          bytes: 1,
          errors: [{ path: "a", code: "EIO" }],
        },
        { type: "fail", now: 1250, code: "AbortError", abort: true },
      ],
    }),
    await scanJobCase({
      name: "timer-cleared-on-finish",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        { type: "progress", now: 1000, bytes: 1 },
        { type: "progress", now: 1100, bytes: 2 },
        { type: "finish", now: 1150, bytes: 3 },
      ],
    }),
    await scanJobCase({
      name: "failure-appends-error",
      startNow: 1000,
      throttleMs: 300,
      steps: [
        {
          type: "progress",
          now: 1000,
          bytes: 1,
          errors: [{ path: "x", code: "EIO" }],
        },
        { type: "fail", now: 1300, code: "ENOSPC" },
      ],
    }),
  ],
  volumeGroups: (() => {
    const cases: Array<{ roots: StorageRootUsage[] }> = [
      { roots: [] },
      {
        roots: [
          {
            id: "home",
            path: "/a",
            volume: { ...volume, deviceId: "1" },
            bytes: 10,
            fileCount: 1,
            categories: [],
          },
          {
            id: "dataBaseDir",
            path: "/b",
            volume: null,
            bytes: 30,
            fileCount: 2,
            categories: [],
          },
          {
            id: "home",
            path: "/c",
            volume: { ...volume, deviceId: "1" },
            bytes: 20,
            fileCount: 3,
            categories: [],
          },
          {
            id: "dataBaseDir",
            path: "/d",
            volume: null,
            bytes: 30,
            fileCount: 4,
            categories: [],
          },
          {
            id: "home",
            path: "/e",
            volume: { ...volume, deviceId: "2" },
            bytes: 5,
            fileCount: 5,
            categories: [],
          },
        ],
      },
    ];
    return cases.map(({ roots }) => ({
      roots,
      expected: groupStorageRootsByVolume(roots),
    }));
  })(),
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
