#!/usr/bin/env node
/* eslint-disable max-lines -- 三方 license 盘点/声明/门禁一体脚本，数据表与校验逻辑集中维护。 */
// 三方 license 盘点/声明/门禁 一体脚本
//
// 用法：
//   pnpm exec tsx scripts/licenses.ts notices   生成 THIRD-PARTY-NOTICES.md（精确版本、原始版权和许可文本）
//   pnpm exec tsx scripts/licenses.ts check     手动门禁：出现 allowlist 之外或许可未知的包即退出码 1
//
// 数据口径：
// - 实装清单 = 全 workspace node_modules 的递归集合，含嵌套版本与符号链接。
// - 声明生成：third-party-npm.mjs 按生产依赖的精确版本收集真实许可文件。
// - prod 判定按锁文件生产图中的精确版本。
// - 商用/半开放检查作用于全量实装包（prod+dev）
import { classifyLicenseExpression } from "./lib/license-classification.js";
import { generateThirdPartyNotices } from "./generate-third-party-notices.mjs";
import { readVerifiedNotices } from "./third-party-notices.mjs";
import { readFile } from "node:fs/promises";
import {
  readWorkspaceProductionGraph,
  scanInstalledPackages,
  missingProductionPackages,
} from "./third-party-npm.mjs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const command = process.argv[2] || "notices";
if (command === "notices") {
  await generateThirdPartyNotices(ROOT);
  process.exit(0);
}

// 标识门禁与声明生成共用 workspace 图和递归扫描，不按包名猜测生产范围。
const { required, projects } = await readWorkspaceProductionGraph(ROOT);
const ownNames = new Set(projects.map((project) => project.name));
const installed = new Map();
const MANUAL_LICENSE = {
  "exif-parser": "MIT", // 包内 LICENSE.md
  khroma: "MIT", // 包内 license 文件
  semaphore: "MIT", // 包内 README License 段
  "css-value": "MIT", // 包内 Readme License 段
  "@fig/autocomplete-helpers": "MIT", // 包内 LICENSE
};
function normLicense(value) {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) return value.map(normLicense).join(" OR ");
  return value?.type?.trim() || "(missing)";
}
const scanned = await scanInstalledPackages(ROOT, projects);
missingProductionPackages(required, scanned);
for (const [key, { pkg }] of scanned) {
  if (ownNames.has(pkg.name)) continue;
  installed.set(key, {
    name: pkg.name,
    version: pkg.version,
    license: normLicense(pkg.license ?? pkg.licenses ?? MANUAL_LICENSE[pkg.name]),
    isProd: required.has(key),
  });
}
// ---------- 分类 ----------
// AND 必须同时满足，OR 可以任选其一；具体规则与严重度顺序见
// docs/specs/license-expression-classification.md。旧的「取首个 AND 成员」兜底会把
// MIT AND GPL-3.0 误判为 green，这里统一交给唯一分类实现。
for (const r of installed.values()) r.bucket = classifyLicenseExpression(r.license);

// ---------- 构建工具许可标识复核（不是二进制发行义务豁免） ----------
const WEAK_ALLOW = [[/^lightningcss/, "当前仅构建依赖；进入生产图时需重新核对 MPL 源码提供义务"]];
function weakAllowReason(r) {
  if (r.isProd) return null;
  for (const [re, why] of WEAK_ALLOW) if (re.test(r.name)) return why;
  return null;
}

if (command === "check") {
  const bad = [];
  for (const r of installed.values()) {
    if (r.bucket === "green") continue;
    if (r.bucket.startsWith("yellow") && weakAllowReason(r)) continue;
    bad.push(r);
  }
  if (bad.length) {
    console.error(`✗ license 检查失败：${bad.length} 个包超出 allowlist：`);
    for (const r of bad.sort((a, b) => a.name.localeCompare(b.name)))
      console.error(`  [${r.bucket}] ${r.name}@${r.version} → ${r.license}`);
    console.error("\n处置：替换依赖，或在 scripts/licenses.ts 的 WEAK_ALLOW 登记人工复核结论。");
    process.exit(1);
  }
  await readVerifiedNotices(ROOT, { requireComplete: process.argv.includes("--strict") });
  const reviewRequired =
    JSON.parse(await readFile(path.join(ROOT, "third-party/inventory.json"), "utf8"))
      .reviewRequired ?? [];
  if (reviewRequired.length)
    console.warn(
      `待补齐/核验材料 ${reviewRequired.length} 项；发布前运行 pnpm exec tsx scripts/licenses.ts check --strict，不得将基础检查通过视为合规完成。`,
    );
  const n = installed.size;
  console.log(
    `✓ 许可标识与声明新鲜度检查通过：${n} 个实装包（构建依赖复核 ${[...installed.values()].filter((r) => r.bucket.startsWith("yellow")).length} 项）；材料限制见 third-party/README.md`,
  );
} else {
  console.error("用法: pnpm exec tsx scripts/licenses.ts [notices|check]");
  process.exit(2);
}
