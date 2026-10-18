#!/usr/bin/env node
// M4/M5 仪表：与 upstream/zcode-baseline 的重合度（逐字节一致 + 品牌归一一致）。
//
// 规则来源：docs/specs/upstream-overlap-metric.md。只输出计数与路径，**绝不输出上游文件内容**
// （clean-room：提示词与上下文不得包含上游实现）。
//
// 用法：
//   pnpm exec tsx scripts/upstream-overlap-check.ts            # report，退出 0
//   pnpm exec tsx scripts/upstream-overlap-check.ts --json     # 机器可读
//   pnpm exec tsx scripts/upstream-overlap-check.ts --list     # 追加两档文件路径
//   pnpm exec tsx scripts/upstream-overlap-check.ts --strict   # M4 终态门禁（headline 必须为 0）

import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_BASELINE = "upstream/zcode-baseline";

export interface TreeEntry {
  mode: string;
  sha: string;
}

export type OverlapTier = "identical" | "normalized" | "distinct";

export interface FileOverlap {
  path: string;
  tier: OverlapTier;
  /** 当前树侧的行数；二进制/非 UTF-8 记 0。 */
  lines: number;
}

export interface OverlapSummary {
  common: number;
  identical: { files: number; lines: number };
  normalized: { files: number; lines: number };
  distinct: { files: number; lines: number };
  oursOnly: number;
  upstreamOnly: number;
  /** 同路径文本文件在当前树侧的总行数（分母）。 */
  commonLines: number;
  /** identical + normalized 的行数（分子）。 */
  overlapLines: number;
  overlapRatio: number;
}

/** 品牌归一 N1：只换品牌词，不动空白/大小写/标点结构。 */
export function normalizeBrand(text: string): string {
  return text
    .replace(/\r\n/g, "\n")
    .replace(/zcodium/gi, "mode")
    .replace(/z-code/gi, "mode")
    .replace(/z_code/gi, "mode")
    .replace(/zcode/gi, "mode");
}

/**
 * 不参与 headline 的豁免前缀：
 * - `third-party/**`、`.agents/skills/**`：clean-room 政策里的 vendored 面；
 * - `scripts/license-texts/**`：第三方许可证原文（台账引用的资产，不是自研代码）。
 * 把第三方复制件算进「自研重合度」会让 0% 永远不可达。
 */
export const EXEMPT_PREFIXES = [
  "third-party/",
  ".agents/skills/",
  "scripts/license-texts/",
] as const;

export function isExempt(path: string): boolean {
  return EXEMPT_PREFIXES.some((prefix) => path.startsWith(prefix));
}

/**
 * M4 范围豁免（2026-10-18 用户裁决，选项 (c)）：Host sidecar / 传输层是 M3 决定复用的
 * Node 运行时，随 Rust 运行时迁移一起清零。单列报告、不进 headline；**不是永久豁免**——
 * Rust 运行时迁移完成后必须移除本列表（见 spec 的重开条件）。
 */
export const DEFERRED_PREFIXES = [
  "packages/services/",
  "packages/shared/",
  "packages/rpc/",
  "packages/server/",
  "packages/provider/",
] as const;

export function isDeferred(path: string): boolean {
  return DEFERRED_PREFIXES.some((prefix) => path.startsWith(prefix));
}

/** 行数：按 `\n` 切分，末尾换行不产生空行；空文本 0 行。 */
export function countLines(text: string): number {
  if (text.length === 0) {
    return 0;
  }
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines.length;
}

export function classifyPair(input: {
  sameSha: boolean;
  oursText: string | null;
  upstreamText: string | null;
}): OverlapTier {
  if (input.sameSha) {
    return "identical";
  }
  if (input.oursText === null || input.upstreamText === null) {
    return "distinct";
  }
  return normalizeBrand(input.oursText) === normalizeBrand(input.upstreamText)
    ? "normalized"
    : "distinct";
}

/** `git ls-tree -r -z` 的输出 → path → entry（只保留 blob；submodule/commit 跳过）。 */
export function parseLsTree(output: Buffer): Map<string, TreeEntry> {
  const entries = new Map<string, TreeEntry>();
  for (const record of output.toString("utf8").split("\0")) {
    if (record.length === 0) {
      continue;
    }
    const tab = record.indexOf("\t");
    if (tab === -1) {
      continue;
    }
    const meta = record.slice(0, tab).split(" ");
    if (meta.length !== 3 || meta[1] !== "blob") {
      continue;
    }
    entries.set(record.slice(tab + 1), { mode: meta[0], sha: meta[2] });
  }
  return entries;
}

export function summarize(
  files: readonly FileOverlap[],
  extras: { oursOnly: number; upstreamOnly: number },
): OverlapSummary {
  const buckets = {
    identical: { files: 0, lines: 0 },
    normalized: { files: 0, lines: 0 },
    distinct: { files: 0, lines: 0 },
  };
  let commonLines = 0;
  for (const file of files) {
    buckets[file.tier].files += 1;
    buckets[file.tier].lines += file.lines;
    commonLines += file.lines;
  }
  const overlapLines = buckets.identical.lines + buckets.normalized.lines;
  return {
    common: files.length,
    identical: buckets.identical,
    normalized: buckets.normalized,
    distinct: buckets.distinct,
    oursOnly: extras.oursOnly,
    upstreamOnly: extras.upstreamOnly,
    commonLines,
    overlapLines,
    overlapRatio: commonLines === 0 ? 0 : overlapLines / commonLines,
  };
}

/** 按顶层目录聚合文件数（只用于人读摘要）。 */
export function topLevelCounts(
  files: readonly { path: string; tier: OverlapTier }[],
): Map<string, Record<OverlapTier, number>> {
  const result = new Map<string, Record<OverlapTier, number>>();
  for (const file of files) {
    const top = file.path.includes("/") ? file.path.slice(0, file.path.indexOf("/")) : "(root)";
    const bucket = result.get(top) ?? { identical: 0, normalized: 0, distinct: 0 };
    bucket[file.tier] += 1;
    result.set(top, bucket);
  }
  return result;
}

function git(args: string[], input?: Buffer): Buffer {
  const result = spawnSync("git", args, {
    cwd: REPO_ROOT,
    input,
    maxBuffer: 1024 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} 失败：${result.stderr.toString("utf8").trim()}`);
  }
  return result.stdout;
}

function resolveBaseline(ref: string): string | null {
  const result = spawnSync("git", ["rev-parse", "--verify", "--quiet", ref], {
    cwd: REPO_ROOT,
    encoding: "utf8",
  });
  return result.status === 0 ? result.stdout.trim() : null;
}

function listTree(ref: string): Map<string, TreeEntry> {
  return parseLsTree(git(["ls-tree", "-r", "-z", ref]));
}

/** 逐条读 blob，回调后立即丢弃字节（不在内存里留全量内容）。 */
async function forEachBlob(
  shas: readonly string[],
  onBlob: (sha: string, bytes: Buffer) => void,
): Promise<void> {
  if (shas.length === 0) {
    return;
  }
  const child = spawn("git", ["cat-file", "--batch"], {
    cwd: REPO_ROOT,
    stdio: ["pipe", "pipe", "inherit"],
  });
  child.stdin.end(`${shas.join("\n")}\n`);

  let pending = Buffer.alloc(0);
  child.stdout.on("data", (chunk: Buffer) => {
    pending = pending.length === 0 ? chunk : Buffer.concat([pending, chunk]);
    for (;;) {
      const headerEnd = pending.indexOf(0x0a);
      if (headerEnd === -1) {
        return;
      }
      const header = pending.subarray(0, headerEnd).toString("utf8").split(" ");
      if (header.length < 3) {
        throw new Error(`无法解析 git cat-file 头：${header.join(" ")}`);
      }
      if (header[1] === "missing") {
        pending = pending.subarray(headerEnd + 1);
        continue;
      }
      const size = Number(header[2]);
      const bodyStart = headerEnd + 1;
      if (pending.length < bodyStart + size + 1) {
        return;
      }
      onBlob(header[0], pending.subarray(bodyStart, bodyStart + size));
      pending = pending.subarray(bodyStart + size + 1);
    }
  });

  await once(child, "close");
}

function decodeText(bytes: Buffer): string | null {
  if (bytes.includes(0)) {
    return null;
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

export interface OverlapReport {
  baseline: string;
  baselineSha: string | null;
  /** M4 计分面（已排除 vendored 与 M4 范围豁免）；headline 只看这一份。 */
  summary: OverlapSummary | null;
  /** vendored / 许可证原文；单列报告，不参与 headline。 */
  excludedSummary: OverlapSummary | null;
  /** M4 范围豁免（sidecar/传输层）；单列报告，随 Rust 运行时迁移清零。 */
  deferredSummary: OverlapSummary | null;
  files: FileOverlap[];
  oursOnlyPaths: string[];
  upstreamOnlyPaths: string[];
}

async function computeReport(baseline: string): Promise<OverlapReport> {
  const sha = resolveBaseline(baseline);
  if (sha === null) {
    return {
      baseline,
      baselineSha: null,
      summary: null,
      excludedSummary: null,
      deferredSummary: null,
      files: [],
      oursOnlyPaths: [],
      upstreamOnlyPaths: [],
    };
  }

  const ours = listTree("HEAD");
  const upstream = listTree(sha);
  const commonPaths = [...ours.keys()].filter((path) => upstream.has(path)).sort();
  const oursOnlyPaths = [...ours.keys()]
    .filter((path) => !upstream.has(path) && !isExempt(path) && !isDeferred(path))
    .sort();
  const upstreamOnlyPaths = [...upstream.keys()]
    .filter((path) => !ours.has(path) && !isExempt(path) && !isDeferred(path))
    .sort();
  const mismatched = commonPaths.filter((path) => ours.get(path)!.sha !== upstream.get(path)!.sha);
  const mismatchShas = new Set(mismatched.map((path) => ours.get(path)!.sha));

  // 先拿当前树侧文本/行数；只有 sha 不同的文件保留文本供归一比较（控制内存）。
  const oursInfo = new Map<string, { lines: number; text: string | null }>();
  await forEachBlob(
    [...new Set(commonPaths.map((path) => ours.get(path)!.sha))],
    (blobSha, bytes) => {
      const text = decodeText(bytes);
      oursInfo.set(blobSha, {
        lines: text === null ? 0 : countLines(text),
        text: mismatchShas.has(blobSha) ? text : null,
      });
    },
  );

  const upstreamText = new Map<string, string | null>();
  await forEachBlob(
    [...new Set(mismatched.map((path) => upstream.get(path)!.sha))],
    (blobSha, bytes) => {
      upstreamText.set(blobSha, decodeText(bytes));
    },
  );

  const files: FileOverlap[] = commonPaths.map((path) => {
    const oursEntry = ours.get(path)!;
    const upstreamEntry = upstream.get(path)!;
    const info = oursInfo.get(oursEntry.sha);
    return {
      path,
      tier: classifyPair({
        sameSha: oursEntry.sha === upstreamEntry.sha,
        oursText: info?.text ?? null,
        upstreamText: upstreamText.get(upstreamEntry.sha) ?? null,
      }),
      lines: info?.lines ?? 0,
    };
  });

  const excludedFiles = files.filter((file) => isExempt(file.path));
  const deferredFiles = files.filter((file) => !isExempt(file.path) && isDeferred(file.path));
  const trackedFiles = files.filter(
    (file) => !isExempt(file.path) && !isDeferred(file.path),
  );

  return {
    baseline,
    baselineSha: sha,
    summary: summarize(trackedFiles, {
      oursOnly: oursOnlyPaths.length,
      upstreamOnly: upstreamOnlyPaths.length,
    }),
    excludedSummary:
      excludedFiles.length === 0
        ? null
        : summarize(excludedFiles, { oursOnly: 0, upstreamOnly: 0 }),
    deferredSummary:
      deferredFiles.length === 0
        ? null
        : summarize(deferredFiles, { oursOnly: 0, upstreamOnly: 0 }),
    files,
    oursOnlyPaths,
    upstreamOnlyPaths,
  };
}

function percent(ratio: number): string {
  return `${(ratio * 100).toFixed(2)}%`;
}

function renderReport(report: OverlapReport, list: boolean): string {
  if (report.summary === null) {
    return `upstream-overlap: baseline unavailable (ref: ${report.baseline})`;
  }
  const summary = report.summary;
  const lines = [
    `upstream-overlap: baseline ${report.baseline} @ ${report.baselineSha?.slice(0, 8)}`,
    `common files: ${summary.common} (ours-only ${summary.oursOnly}, upstream-only ${summary.upstreamOnly})`,
    `identical: ${summary.identical.files} files / ${summary.identical.lines} lines`,
    `normalized: ${summary.normalized.files} files / ${summary.normalized.lines} lines`,
    `distinct: ${summary.distinct.files} files / ${summary.distinct.lines} lines`,
    `headline: ${percent(summary.overlapRatio)} (${summary.overlapLines} of ${summary.commonLines} lines)`,
  ];
  if (report.excludedSummary) {
    const excluded = report.excludedSummary;
    lines.push(
      `excluded (from headline): ${excluded.common} files / ${excluded.commonLines} lines ` +
        `(identical ${excluded.identical.files}, normalized ${excluded.normalized.files})`,
    );
  }
  if (report.deferredSummary) {
    const deferred = report.deferredSummary;
    lines.push(
      `deferred (M4 scope exception; cleared with the Rust runtime migration): ` +
        `${deferred.common} files / ${deferred.commonLines} lines ` +
        `(identical ${deferred.identical.files}, normalized ${deferred.normalized.files})`,
    );
  }
  for (const [top, bucket] of [...topLevelCounts(report.files)].sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    lines.push(
      `  ${top}: identical ${bucket.identical}, normalized ${bucket.normalized}, distinct ${bucket.distinct}`,
    );
  }
  if (list) {
    for (const tier of ["identical", "normalized"] as const) {
      lines.push(`${tier} paths:`);
      for (const file of report.files.filter((entry) => entry.tier === tier)) {
        lines.push(`  ${file.path}`);
      }
    }
  }
  return lines.join("\n");
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const json = args.includes("--json");
  const list = args.includes("--list");
  const strict = args.includes("--strict");
  const baselineIndex = args.indexOf("--baseline");
  const baseline = baselineIndex >= 0 ? args[baselineIndex + 1] : DEFAULT_BASELINE;

  const report = await computeReport(baseline);
  if (json) {
    process.stdout.write(
      `${JSON.stringify(
        {
          baseline: report.baseline,
          baselineSha: report.baselineSha,
          summary: report.summary,
          excluded: report.excludedSummary,
          deferred: report.deferredSummary,
          oursOnly: report.oursOnlyPaths,
          upstreamOnly: report.upstreamOnlyPaths,
          files: report.files,
          identical: report.files.filter((file) => file.tier === "identical").map((file) => file.path),
          normalized: report.files.filter((file) => file.tier === "normalized").map((file) => file.path),
        },
        null,
        2,
      )}\n`,
    );
  } else {
    process.stdout.write(`${renderReport(report, list)}\n`);
  }

  if (strict) {
    if (report.summary === null) {
      process.exitCode = 2;
    } else if (report.summary.overlapLines > 0) {
      process.exitCode = 1;
    }
  }
}

const entry = process.argv[1] ? resolve(process.argv[1]) : "";
if (entry === fileURLToPath(import.meta.url)) {
  main().catch((error: unknown) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
