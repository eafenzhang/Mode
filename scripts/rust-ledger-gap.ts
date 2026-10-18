#!/usr/bin/env node
// M4 第三方台账覆盖门禁（T2）：每个发布目标的解析图里，所有第三方 crate 必须出现在
// third-party/rust-sources.json 中。
//
// 背景与缺口测量：docs/plans/2026-10-18-m4-third-party-ledger.md。
// 当前是 **report 模式**：台账只覆盖 CUA helper 的 63 个 crate，T3（Linux CI 重生成）完成前
// 必然有缺项；`--strict` 供 T3 之后接成阻断门禁。
//
// 用法：
//   pnpm check:rust-ledger                         # report（cargo metadata 由本脚本调用）
//   pnpm check:rust-ledger -- --metadata meta.json # 用已生成的 metadata（本机无 cargo 时）
//   pnpm check:rust-ledger -- --json
//   pnpm check:rust-ledger -- --strict             # 有缺项时退出 1

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const LEDGER_PATH = "third-party/rust-sources.json";

/** 不发布到任何产物、不参与台账覆盖的 workspace 成员。 */
export const NON_SHIPPED_WORKSPACE = ["tauri-test-manifest", "updater-verify"] as const;

export interface LedgerComponent {
  id: string;
  version: string;
  license: string;
}

export interface CargoPackage {
  id: string;
  name: string;
  version: string;
  license: string | null;
  source: string | null;
}

export interface CargoMetadata {
  packages: CargoPackage[];
  resolve: {
    nodes: Array<{ id: string; deps: Array<{ pkg: string }> }>;
  };
}

export interface TargetAudit {
  target: string;
  external: number;
  covered: number;
  missing: string[];
  versionDrift: string[];
}

export interface LedgerAudit {
  ledgerComponents: number;
  targets: TargetAudit[];
  union: { external: number; covered: number; missing: string[]; versionDrift: string[] };
  unknownLicense: string[];
}

function key(name: string, version: string): string {
  return `${name}@${version}`;
}

/** 台账索引：按 name@version 精确匹配，另按 name 记录已知版本以区分「完全缺失」与「版本漂移」。 */
export function indexLedger(components: readonly LedgerComponent[]): {
  exact: Set<string>;
  versionsByName: Map<string, Set<string>>;
} {
  const exact = new Set<string>();
  const versionsByName = new Map<string, Set<string>>();
  for (const component of components) {
    exact.add(key(component.id, component.version));
    const versions = versionsByName.get(component.id) ?? new Set<string>();
    versions.add(component.version);
    versionsByName.set(component.id, versions);
  }
  return { exact, versionsByName };
}

/** 默认发布目标：除显式非发布成员外的全部 workspace crate。 */
export function defaultTargets(metadata: CargoMetadata): string[] {
  const workspaceIds = new Set(metadata.packages.filter((p) => p.source === null).map((p) => p.id));
  const roots = new Set<string>();
  for (const node of metadata.resolve.nodes) {
    if (workspaceIds.has(node.id)) {
      const pkg = metadata.packages.find((p) => p.id === node.id);
      if (pkg && !NON_SHIPPED_WORKSPACE.includes(pkg.name as (typeof NON_SHIPPED_WORKSPACE)[number])) {
        roots.add(pkg.name);
      }
    }
  }
  return [...roots].sort();
}

/** 从根 crate 名出发，返回可达 package id 集合。 */
export function collectReachable(metadata: CargoMetadata, targets: readonly string[]): Set<string> {
  const byName = new Map<string, string[]>();
  for (const pkg of metadata.packages) {
    const list = byName.get(pkg.name) ?? [];
    list.push(pkg.id);
    byName.set(pkg.name, list);
  }
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const seen = new Set<string>();
  const stack: string[] = [];
  for (const target of targets) {
    const ids = byName.get(target);
    if (!ids || ids.length === 0) {
      throw new Error(`workspace 目标不存在：${target}`);
    }
    stack.push(...ids);
  }
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) {
      continue;
    }
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      stack.push(dep.pkg);
    }
  }
  return seen;
}

export function auditLedger(
  metadata: CargoMetadata,
  components: readonly LedgerComponent[],
  targets: readonly string[] = defaultTargets(metadata),
): LedgerAudit {
  const ledger = indexLedger(components);
  const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const unknownLicense: string[] = [];

  const targetsAudit = targets.map((target) => {
    const reachable = collectReachable(metadata, [target]);
    const external = [...reachable]
      .map((id) => packages.get(id))
      .filter((pkg): pkg is CargoPackage => Boolean(pkg && pkg.source !== null));
    const missing: string[] = [];
    const versionDrift: string[] = [];
    for (const pkg of external) {
      if (!pkg.license) {
        unknownLicense.push(key(pkg.name, pkg.version));
      }
      if (ledger.exact.has(key(pkg.name, pkg.version))) {
        continue;
      }
      const label = `${pkg.name}@${pkg.version} [${pkg.license ?? "NO-LICENSE"}]`;
      missing.push(label);
      if (ledger.versionsByName.has(pkg.name)) {
        versionDrift.push(label);
      }
    }
    return {
      target,
      external: external.length,
      covered: external.length - missing.length,
      missing: missing.sort(),
      versionDrift: versionDrift.sort(),
    };
  });

  // 并集：同一个 crate 被多个目标共享时只算一次。
  const unionReachable = collectReachable(metadata, targets);
  const unionExternal = [...unionReachable]
    .map((id) => packages.get(id))
    .filter((pkg): pkg is CargoPackage => Boolean(pkg && pkg.source !== null));
  const unionMissing = unionExternal
    .filter((pkg) => !ledger.exact.has(key(pkg.name, pkg.version)))
    .map((pkg) => `${pkg.name}@${pkg.version} [${pkg.license ?? "NO-LICENSE"}]`)
    .sort();

  return {
    ledgerComponents: components.length,
    targets: targetsAudit,
    union: {
      external: unionExternal.length,
      covered: unionExternal.length - unionMissing.length,
      missing: unionMissing,
      versionDrift: unionMissing
        .filter((label) => ledger.versionsByName.has(label.slice(0, label.indexOf("@"))))
        .sort(),
    },
    unknownLicense: [...new Set(unknownLicense)].sort(),
  };
}

function loadMetadata(path: string | null): CargoMetadata {
  if (path) {
    return JSON.parse(readFileSync(path, "utf8")) as CargoMetadata;
  }
  const result = spawnSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(
      `cargo metadata 失败（可先用 node scripts/rust-cargo.ts metadata --format-version 1 --locked 生成文件再传 --metadata）：\n${result.stderr}`,
    );
  }
  return JSON.parse(result.stdout) as CargoMetadata;
}

function render(audit: LedgerAudit): string {
  const lines = [`rust-ledger-gap: ledger ${audit.ledgerComponents} crates`];
  for (const target of audit.targets) {
    lines.push(
      `  ${target.target}: external ${target.external}, covered ${target.covered}, missing ${target.missing.length}` +
        (target.versionDrift.length > 0 ? `, version-drift ${target.versionDrift.length}` : ""),
    );
  }
  lines.push(
    `union: external ${audit.union.external}, covered ${audit.union.covered}, missing ${audit.union.missing.length}` +
      (audit.union.versionDrift.length > 0
        ? `, version-drift ${audit.union.versionDrift.length}`
        : ""),
  );
  if (audit.unknownLicense.length > 0) {
    lines.push(`unknown license identifiers (${audit.unknownLicense.length}):`);
    for (const entry of audit.unknownLicense) {
      lines.push(`  ${entry}`);
    }
  }
  if (audit.union.missing.length > 0) {
    lines.push("missing (first 20):");
    for (const entry of audit.union.missing.slice(0, 20)) {
      lines.push(`  ${entry}`);
    }
    if (audit.union.missing.length > 20) {
      lines.push(`  … ${audit.union.missing.length - 20} more (use --json)`);
    }
  }
  return lines.join("\n");
}

function main(): void {
  const args = process.argv.slice(2);
  const metadataIndex = args.indexOf("--metadata");
  const metadataPath = metadataIndex >= 0 ? args[metadataIndex + 1] : null;
  const json = args.includes("--json");
  const strict = args.includes("--strict");
  const rootsIndex = args.indexOf("--roots");
  const roots = rootsIndex >= 0 ? args[rootsIndex + 1].split(",").filter(Boolean) : undefined;

  const metadata = loadMetadata(metadataPath);
  const ledger = JSON.parse(readFileSync(resolve(REPO_ROOT, LEDGER_PATH), "utf8")) as {
    components: LedgerComponent[];
  };
  const audit = auditLedger(metadata, ledger.components, roots);

  if (json) {
    process.stdout.write(`${JSON.stringify(audit, null, 2)}\n`);
  } else {
    process.stdout.write(`${render(audit)}\n`);
  }

  if (strict && (audit.union.missing.length > 0 || audit.unknownLicense.length > 0)) {
    process.exitCode = 1;
  }
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
