#!/usr/bin/env node
// M4 T1：从 cargo metadata + cargo registry 缓存生成候选 Rust 第三方台账。
//
// 输入：
//   - 根 workspace 的 `cargo metadata`（发布目标 = 全部成员，排除测试/校验专用成员）；
//   - `crates/mode-cua-ax` 的 `cargo metadata`（它不在根 workspace 里，单独解析）；
//   - 两份 Cargo.lock 的 checksum；
//   - `$CARGO_HOME/registry/src/*/<name>-<version>/` 里的许可证原文。
//
// 输出（默认 `.plan/rust-ledger-candidate/`，**不覆盖入库台账**）：
//   - `rust-sources.json`（schema 与 third-party/rust-sources.json 兼容，另加 `targets`）；
//   - `license-texts/<sha256>.txt`（CRLF 归一后按内容哈希命名）。
//
// 生成后自动用 T2 的审计函数自校验：两份图的第三方 crate 必须全部覆盖，否则退出 1。
// 真正的入库与 notices 重生成仍走 Linux CI + 人工 review（见 M4 台账计划 T3）。
//
// 用法：
//   node scripts/rust-cargo.ts metadata --format-version 1 --locked > .plan/root-meta.json
//   node scripts/rust-cargo.ts metadata --manifest-path crates/mode-cua-ax/Cargo.toml \
//     --format-version 1 --locked > .plan/cua-meta.json
//   pnpm exec tsx scripts/generate-rust-ledger.ts \
//     --root-metadata .plan/root-meta.json --cua-metadata .plan/cua-meta.json \
//     --out .plan/rust-ledger-candidate

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  auditLedger,
  collectReachable,
  defaultTargets,
  type CargoMetadata,
  type LedgerComponent,
} from "./rust-ledger-gap.ts";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CUA_MANIFEST = "crates/mode-cua-ax/Cargo.toml";
const CUA_TARGET = "mode-cua-ax";

export interface NoticeRecord {
  member: string;
  file: string;
  sha256: string;
}

export interface LedgerEntry extends LedgerComponent {
  checksum: string | null;
  notices: NoticeRecord[];
  reviewRequired?: boolean;
  /** `package` = 包内文件；`standard` = 包内没有，按声明标识附仓库标准文本。 */
  licenseTextSource?: "package" | "standard";
  /** 人工 review 时需要看的说明（缺失原因/待决点）。 */
  reviewNote?: string;
}

export interface LockPackage {
  name: string;
  version: string;
  checksum: string | null;
}

/** 解析 Cargo.lock 的 `[[package]]` 块（只取 name/version/checksum）。 */
export function parseLockPackages(lockText: string): LockPackage[] {
  const packages: LockPackage[] = [];
  for (const block of lockText.split("[[package]]").slice(1)) {
    const name = /^name = "(.*)"$/m.exec(block)?.[1];
    const version = /^version = "(.*)"$/m.exec(block)?.[1];
    if (!name || !version) {
      continue;
    }
    packages.push({
      name,
      version,
      checksum: /^checksum = "(.*)"$/m.exec(block)?.[1] ?? null,
    });
  }
  return packages;
}

const NON_LICENSE_FILE = /\.(rs|toml|json|lock|ya?ml)$/i;

/** 目录项里哪些文件是许可证原文（大小写不敏感；只认根目录文件名；排除代码/配置）。 */
export function licenseMemberNames(fileNames: readonly string[]): string[] {
  return fileNames
    .filter(
      (name) =>
        /^(un)?licen[cs]e|^(copying|copyright|notice)/i.test(name) &&
        !NON_LICENSE_FILE.test(name),
    )
    .sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
}

export function sha256Hex(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** SPDX 表达式的标识符（`MIT OR Apache-2.0`、`MIT/Apache-2.0` 两种写法都拆）。 */
export function parseSpdxIdentifiers(expression: string): string[] {
  return expression
    .split(/\s+(?:OR|AND)\s+|\//i)
    .map((part) => part.trim().replace(/^\(|\)$/g, "").trim())
    .filter(Boolean);
}

/**
 * 可自动附文本的标识：LGPL 是**多选一里的一个选项**，未明确选择前不附（避免被读成已选
 * LGPL）；MPL 是必附的弱互惠文本，照附但保留人工 review（源码可得性/措辞）。
 */
export function selectableLicenseIdentifiers(expression: string): string[] {
  return parseSpdxIdentifiers(expression).filter((identifier) => !/^LGPL/i.test(identifier));
}

/** 该表达式是否仍需人工判断（MPL 履约或 LGPL 选择）。 */
export function requiresHumanReview(expression: string): boolean {
  return /(?:MPL|LGPL)/i.test(expression);
}

/** 按正文特征识别许可证文本；识别不出返回 null（调用方保留 reviewRequired）。 */
export function classifyLicenseText(text: string): string | null {
  const normalized = text.toLowerCase();
  if (normalized.includes("mozilla public license") && normalized.includes("version 2.0")) {
    return "MPL-2.0";
  }
  if (
    normalized.includes("gnu lesser general public license") &&
    normalized.includes("version 2.1")
  ) {
    return "LGPL-2.1-or-later";
  }
  if (normalized.includes("apache license") && normalized.includes("version 2.0")) {
    return "Apache-2.0";
  }
  if (normalized.includes("redistribution and use in source and binary forms")) {
    return normalized.includes("neither the name") ? "BSD-3-Clause" : "BSD-2-Clause";
  }
  if (normalized.includes("this software is provided 'as-is'") ||
      normalized.includes('this software is provided "as-is"')) {
    return "Zlib";
  }
  if (normalized.includes("permission is hereby granted, free of charge")) {
    return "MIT";
  }
  if (
    normalized.includes("permission to use, copy, modify, and/or distribute this software") &&
    normalized.includes("provided that the above copyright notice")
  ) {
    return "ISC";
  }
  if (normalized.includes("permission to use, copy, modify, and/or distribute this software")) {
    return "0BSD";
  }
  if (normalized.includes("this is free and unencumbered software released into the public domain")) {
    return "Unlicense";
  }
  return null;
}

export interface StandardText {
  file: string;
  sha256: string;
}

/** 扫描 `scripts/license-texts/`，按正文特征建立 标识符 → 文本 的索引。 */
export function indexStandardLicenseTexts(directory: string): Map<string, StandardText> {
  const index = new Map<string, StandardText>();
  for (const fileName of readdirSync(directory)) {
    if (!fileName.endsWith(".txt")) {
      continue;
    }
    const bytes = readFileSync(join(directory, fileName));
    const identifier = classifyLicenseText(bytes.toString("utf8"));
    if (identifier && !index.has(identifier)) {
      index.set(identifier, {
        file: `scripts/license-texts/${fileName}`,
        sha256: sha256Hex(bytes),
      });
    }
  }
  return index;
}

function loadMetadata(path: string | null, manifestPath?: string): CargoMetadata {
  if (path) {
    return JSON.parse(readFileSync(path, "utf8")) as CargoMetadata;
  }
  const args = ["metadata", "--format-version", "1", "--locked"];
  if (manifestPath) {
    args.push("--manifest-path", manifestPath);
  }
  const result = spawnSync("cargo", args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(
      `cargo metadata 失败（可用 node scripts/rust-cargo.ts metadata … 生成文件再传参）：\n${result.stderr}`,
    );
  }
  return JSON.parse(result.stdout) as CargoMetadata;
}

function loadLock(cwd: string): LockPackage[] {
  return parseLockPackages(readFileSync(join(cwd, "Cargo.lock"), "utf8"));
}

/** registry 源目录索引：`<name>-<version>` → 解包目录。 */
function indexRegistrySources(): Map<string, string> {
  const cargoHome = process.env.CARGO_HOME ?? join(homedir(), ".cargo");
  const registryRoot = join(cargoHome, "registry", "src");
  const index = new Map<string, string>();
  if (!existsSync(registryRoot)) {
    return index;
  }
  for (const registry of readdirSync(registryRoot)) {
    const registryDir = join(registryRoot, registry);
    for (const dir of readdirSync(registryDir)) {
      const full = join(registryDir, dir);
      if (existsSync(join(full, "Cargo.toml"))) {
        index.set(dir, full);
      }
    }
  }
  return index;
}

interface BuildResult {
  entries: LedgerEntry[];
  writtenNotices: Map<string, string>;
  missingSource: string[];
  standardAttached: number;
}

function buildEntries(
  packages: ReadonlyArray<{ name: string; version: string; license: string; source: string | null }>,
  checksums: Map<string, string>,
  registry: Map<string, string>,
  standardTexts: Map<string, StandardText>,
  noticesDir: string,
  writtenNotices: Map<string, string>,
): BuildResult {
  const entries: LedgerEntry[] = [];
  const missingSource: string[] = [];
  let standardAttached = 0;

  for (const pkg of packages) {
    const dir = registry.get(`${pkg.name}-${pkg.version}`);
    if (!dir) {
      missingSource.push(`${pkg.name}@${pkg.version}`);
      continue;
    }
    const members = licenseMemberNames(readdirSync(dir));
    const notices: NoticeRecord[] = [];
    for (const member of members) {
      const raw = readFileSync(join(dir, member));
      // 与 generator 的输入哈希同口径：CRLF 归一后按内容哈希命名，文本字节保留。
      const bytes = Buffer.from(raw.toString("utf8").replaceAll("\r\n", "\n"));
      const sha = sha256Hex(bytes);
      let file = writtenNotices.get(sha);
      if (!file) {
        file = `scripts/license-texts/${sha}.txt`;
        mkdirSync(noticesDir, { recursive: true });
        writeFileSync(join(noticesDir, `${sha}.txt`), bytes);
        writtenNotices.set(sha, file);
      }
      notices.push({ member, file, sha256: sha });
    }

    let reviewRequired = false;
    let licenseTextSource: "package" | "standard" | undefined;
    let reviewNote: string | undefined;
    if (notices.length > 0) {
      licenseTextSource = "package";
    } else {
      const identifiers = selectableLicenseIdentifiers(pkg.license);
      const missing = identifiers.filter((identifier) => !standardTexts.has(identifier));
      if (identifiers.length > 0 && missing.length === 0) {
        for (const identifier of identifiers) {
          const text = standardTexts.get(identifier)!;
          notices.push({
            member: `LICENSE-${identifier.replace(/[^A-Za-z0-9.-]/g, "-")}`,
            file: text.file,
            sha256: text.sha256,
          });
        }
        standardAttached += 1;
        licenseTextSource = "standard";
      }
      if (requiresHumanReview(pkg.license)) {
        reviewRequired = true;
        reviewNote = /MPL/i.test(pkg.license)
          ? "no licence file; MPL-2.0 text attached, source-availability/notice wording requires human review"
          : "no licence file; permissive texts attached, LGPL option not elected — license choice requires human review";
      } else if (missing.length > 0 || identifiers.length === 0) {
        reviewRequired = true;
        reviewNote =
          missing.length > 0
            ? `no licence file; missing standard text for ${missing.join(", ")}`
            : "no licence file; no usable license identifier";
      } else {
        reviewNote =
          "no licence file in package; standard text attached from repository corpus";
      }
    }

    entries.push({
      id: pkg.name,
      version: pkg.version,
      license: pkg.license,
      checksum: checksums.get(`${pkg.name}@${pkg.version}`) ?? null,
      notices,
      ...(reviewRequired ? { reviewRequired: true } : {}),
      ...(licenseTextSource ? { licenseTextSource } : {}),
      ...(reviewNote ? { reviewNote } : {}),
    });
  }
  return { entries, writtenNotices, missingSource, standardAttached };
}

function main(): void {
  const args = process.argv.slice(2);
  const arg = (name: string): string | null => {
    const index = args.indexOf(name);
    return index >= 0 ? args[index + 1] : null;
  };
  const outDir = resolve(REPO_ROOT, arg("--out") ?? ".plan/rust-ledger-candidate");
  const rootMetadataPath = arg("--root-metadata");
  const cuaMetadataPath = arg("--cua-metadata");

  const rootMetadata = loadMetadata(rootMetadataPath);
  const cuaMetadata = loadMetadata(
    cuaMetadataPath,
    cuaMetadataPath ? undefined : CUA_MANIFEST,
  );
  const targets = defaultTargets(rootMetadata);

  const rootPackages = new Map(rootMetadata.packages.map((pkg) => [pkg.id, pkg]));
  const cuaPackages = new Map(cuaMetadata.packages.map((pkg) => [pkg.id, pkg]));

  // 直接复用 T2 的可达闭包，避免两份实现漂移。
  const rootExternal = collectExternalFromMetadata(rootMetadata, targets, rootPackages);
  const cuaExternal = collectExternalFromMetadata(cuaMetadata, [CUA_TARGET], cuaPackages);
  const unique = new Map<string, { name: string; version: string; license: string; source: string | null }>();
  for (const pkg of [...rootExternal, ...cuaExternal]) {
    unique.set(`${pkg.name}@${pkg.version}`, pkg);
  }

  const checksums = new Map<string, string>();
  for (const pkg of [...loadLock(REPO_ROOT), ...loadLock(join(REPO_ROOT, "crates/mode-cua-ax"))]) {
    if (pkg.checksum) {
      checksums.set(`${pkg.name}@${pkg.version}`, pkg.checksum);
    }
  }

  const registry = indexRegistrySources();
  const standardTexts = indexStandardLicenseTexts(
    join(REPO_ROOT, "scripts", "license-texts"),
  );
  const noticesDir = join(outDir, "license-texts");
  const { entries, writtenNotices, missingSource, standardAttached } = buildEntries(
    [...unique.values()].sort((left, right) =>
      left.name !== right.name
        ? left.name < right.name
          ? -1
          : 1
        : left.version < right.version
          ? -1
          : left.version > right.version
            ? 1
            : 0,
    ),
    checksums,
    registry,
    standardTexts,
    noticesDir,
    new Map(),
  );

  if (missingSource.length > 0) {
    process.stderr.write(
      `registry 缓存缺少 ${missingSource.length} 个 crate 的源码目录（先 cargo fetch）：\n` +
        `${missingSource.slice(0, 20).join("\n")}\n`,
    );
    process.exitCode = 1;
    return;
  }

  const ledger = {
    schemaVersion: 1,
    scope:
      "Crates reachable from the shipped Rust targets (all workspace members except " +
      "tauri-test-manifest/updater-verify) plus crates/mode-cua-ax (separate lockfile). " +
      "Like the native-search ledger this is a conservative union over the full Cargo.lock, " +
      "covering transitive, build-time and platform-conditional crates; it is not a claim that " +
      "every crate is linked into every build. Licence identifiers come from the locked crate " +
      "manifests; notice files are byte-preserved licence texts (CRLF normalised to LF). Crates " +
      "that ship no licence file are marked reviewRequired.",
    targets: [...targets, CUA_TARGET].sort(),
    components: entries,
  };
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, "rust-sources.json"), `${JSON.stringify(ledger, null, 2)}\n`);

  // 自校验：两份依赖图都必须被生成的台账完全覆盖。
  const rootAudit = auditLedger(rootMetadata, entries, targets);
  const cuaAudit = auditLedger(cuaMetadata, entries, [CUA_TARGET]);
  const missing = rootAudit.union.missing.length + cuaAudit.union.missing.length;
  const unknown = rootAudit.unknownLicense.length + cuaAudit.unknownLicense.length;
  const reviewRequired = entries.filter((entry) => entry.reviewRequired).length;

  process.stdout.write(
    [
      `rust-ledger: components ${entries.length}（root targets ${targets.length} + ${CUA_TARGET}）`,
      `license texts ${writtenNotices.size} unique`,
      `standard texts attached ${standardAttached}`,
      `reviewRequired ${reviewRequired}`,
      `self-check: root missing ${rootAudit.union.missing.length}, cua missing ${cuaAudit.union.missing.length}, unknown-license ${unknown}`,
      `written ${join(outDir, "rust-sources.json")}`,
    ].join("\n") + "\n",
  );
  if (missing > 0 || unknown > 0) {
    process.exitCode = 1;
  }
}

/**
 * 从 metadata 的可达闭包里取第三方 crate。import 自 T2 的 collectReachable，
 * 避免「审计」与「生成」两份可达性实现漂移。
 */
function collectExternalFromMetadata(
  metadata: CargoMetadata,
  targets: string[],
  packages: Map<string, CargoMetadata["packages"][number]>,
): Array<{ name: string; version: string; license: string; source: string | null }> {
  const reachable = collectReachable(metadata, targets);
  return [...reachable]
    .map((id) => packages.get(id))
    .filter(
      (pkg): pkg is CargoMetadata["packages"][number] =>
        Boolean(pkg && pkg.source !== null && pkg.license),
    )
    .map((pkg) => ({
      name: pkg.name,
      version: pkg.version,
      license: pkg.license!,
      source: pkg.source,
    }));
}

const entry = process.argv[1] ? resolve(process.argv[1]) : "";
if (entry === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
