#!/usr/bin/env node
// M2 Task 7：对拍矩阵的分类门禁（不是行覆盖率）。
//
// 规则来源：docs/specs/agent-runtime-parity-matrix.md「CI 门禁」一节。
// 1) packages/services/test/*.test.ts 每个文件必须命中一条分类规则，否则退出 1——
//    防止新增测试随文件增长静默脱离矩阵；
// 2) 运行时对拍资产清单里的路径必须存在（目录按目录存在性判定），删除/改名未同步退出 1。
//
// 用法：
//   pnpm exec tsx scripts/agent-runtime-parity-matrix.ts --check
//   pnpm exec tsx scripts/agent-runtime-parity-matrix.ts --json
//
// 语言政策：新增脚本只用 .ts（本文件）；不要改成 .mjs。

import { existsSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SERVICES_TEST_DIR = "packages/services/test";

export type ParityVerdict = "not-applicable" | "not-translatable";

export interface Classification {
  domain: string;
  verdict: ParityVerdict;
  reason: string;
}

interface ClassificationRule extends Classification {
  /** 互斥：文件名单条规则只提供一种匹配方式。 */
  match?: RegExp;
  files?: ReadonlySet<string>;
}

/** 与 spec 的表格一一对应；改规则前先改 spec。 */
const RULES: readonly ClassificationRule[] = [
  {
    domain: "bot",
    verdict: "not-applicable",
    reason: "IM bot 业务；M2 不含 bot 宿主（M3 由 sidecar 复用）",
    match: /^bot-.*\.test\.ts$/,
  },
  {
    domain: "cua",
    verdict: "not-applicable",
    reason: "电脑控制独立线",
    match: /^cua-.*\.test\.ts$/,
  },
  {
    domain: "data",
    verdict: "not-applicable",
    reason: "存储路径/迁移；归 Host（M3）",
    match: /^(dataRoot|dataBaseDir).*\.test\.ts$/,
  },
  {
    domain: "plugin",
    verdict: "not-applicable",
    reason: "插件市场（M3）",
    match: /^(plugin-|official-plugin-attribution).*\.test\.ts$/,
  },
  {
    domain: "provider",
    verdict: "not-applicable",
    reason: "provider 配置层；Rust 侧目前只有脚本化 fake（DR-T5-2）",
    match: /^(provider|remoteModelCatalog|retiredZhipu|orcarouter).*\.test\.ts$/,
  },
  {
    domain: "host-untranslatable",
    verdict: "not-translatable",
    reason: "镜像/会话存储/宿主层；不是 stdio 运行时事件面",
    files: new Set([
      "conversationMirrorService.test.ts",
      "importedClaudeRecovery.test.ts",
      "nonCliAcpRetirement.test.ts",
      "conversation-typing-bubble.test.ts",
    ]),
  },
  {
    domain: "misc",
    verdict: "not-applicable",
    reason: "凭据 / 通道注册 / CUA 根",
    files: new Set([
      "credential-service.test.ts",
      "wecom-registration.test.ts",
      "weixin-provider-self-id.test.ts",
      "windowsCuaDevRootContract.test.ts",
    ]),
  },
];

/**
 * 运行时对拍资产的入口清单。目录级资产（oracle fixtures）按目录存在性检查。
 * 删改这些文件时同步更新本清单与 spec，否则 CI 会红。
 */
export const PARITY_ASSETS: readonly string[] = [
  "crates/agent-core/tests/tool_parity.rs",
  "crates/agent-core/tests/turn_stream.rs",
  "crates/agent-core/tests/task_control.rs",
  "crates/agent-core/tests/task_notification.rs",
  "crates/agent-core/tests/provider_script.rs",
  "crates/agent-core/tests/session_turn.rs",
  "crates/agent-core/tests/stream_parity.rs",
  "crates/agent-tools/tests/read_only.rs",
  "crates/agent-protocol/tests/fixtures.rs",
  "crates/agent-protocol/tests/progress.rs",
  "packages/desktop/tests/agent-runtime-oracle/fixtures",
];

export function classifyServicesTestFile(fileName: string): Classification | null {
  for (const rule of RULES) {
    const hit = rule.files?.has(fileName) ?? rule.match?.test(fileName) ?? false;
    if (hit) {
      return { domain: rule.domain, verdict: rule.verdict, reason: rule.reason };
    }
  }
  return null;
}

export interface ParityReport {
  services: {
    total: number;
    classified: number;
    unclassified: string[];
    byDomain: Record<string, { count: number; verdict: ParityVerdict; reason: string }>;
  };
  assets: {
    declared: string[];
    missing: string[];
  };
  ok: boolean;
}

export function buildParityReport(input: {
  servicesTestFiles: readonly string[];
  assetExists: (path: string) => boolean;
}): ParityReport {
  const byDomain: ParityReport["services"]["byDomain"] = {};
  const unclassified: string[] = [];

  for (const fileName of input.servicesTestFiles) {
    const classification = classifyServicesTestFile(fileName);
    if (!classification) {
      unclassified.push(fileName);
      continue;
    }
    const bucket = byDomain[classification.domain];
    if (bucket) {
      bucket.count += 1;
    } else {
      byDomain[classification.domain] = {
        count: 1,
        verdict: classification.verdict,
        reason: classification.reason,
      };
    }
  }

  const missing = PARITY_ASSETS.filter((asset) => !input.assetExists(asset));
  const total = input.servicesTestFiles.length;
  const classified = total - unclassified.length;

  return {
    services: { total, classified, unclassified, byDomain },
    assets: { declared: [...PARITY_ASSETS], missing },
    ok: unclassified.length === 0 && missing.length === 0,
  };
}

function renderReport(report: ParityReport): string {
  const lines = [
    `[parity-matrix] services tests: ${report.services.total} ` +
      `(classified ${report.services.classified}, unclassified ${report.services.unclassified.length})`,
  ];
  for (const [domain, bucket] of Object.entries(report.services.byDomain)) {
    lines.push(`  ${domain}: ${bucket.count} (${bucket.verdict}) — ${bucket.reason}`);
  }
  if (report.services.unclassified.length > 0) {
    lines.push("  未分类文件（补 spec 分类规则后再提交）：");
    for (const fileName of report.services.unclassified) {
      lines.push(`    - ${fileName}`);
    }
  }
  lines.push(
    `[parity-matrix] runtime assets: ${report.assets.declared.length - report.assets.missing.length}` +
      `/${report.assets.declared.length} present`,
  );
  for (const asset of report.assets.missing) {
    lines.push(`  缺失资产（sync spec / PARITY_ASSETS）：${asset}`);
  }
  return lines.join("\n");
}

function main(): void {
  const servicesTestFiles = readdirSync(join(REPO_ROOT, SERVICES_TEST_DIR))
    .filter((fileName) => fileName.endsWith(".test.ts"))
    .sort((left, right) => left.localeCompare(right));

  const report = buildParityReport({
    servicesTestFiles,
    assetExists: (asset) => existsSync(join(REPO_ROOT, asset)),
  });

  if (process.argv.includes("--json")) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } else {
    process.stdout.write(`${renderReport(report)}\n`);
  }

  if (!report.ok) {
    process.exitCode = 1;
  }
}

const entry = process.argv[1] ? resolve(process.argv[1]) : "";
if (entry === fileURLToPath(import.meta.url)) {
  main();
}
