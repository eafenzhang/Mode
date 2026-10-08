#!/usr/bin/env node
// 语言政策门禁 —— 扫描 git 索引里的受管文件，非 vendored 路径新增禁用扩展名即失败。
// 规则来源：docs/specs/language-policy.md。
//
// 三种语义：
//   1) 新增违规 → fail：命中禁用扩展名、不属豁免前缀、不在允许清单 → 打印路径与计数，退出 1；
//   2) vendored 豁免 → skip：.agents/skills/、third-party/ 前缀不检查（台账 + linguist-vendored 依据）；
//   3) 清单陈旧 → warn 不 fail：允许清单里的文件已从索引删除，只打印告警，退出码仍 0。
//
// 裁定由来：R2 —— 扫描口径是 `git ls-files`（索引全量），不是计划原文的
// `git diff --cached`（在 pre-push/CI 接线点会空转）；R7 —— 陈旧清单条目只告警
// 不失败，只有新增违规才退出 1。
//
// 用法：node scripts/check-language-policy.mjs（以 import.meta.url 定位仓库根，
// 任意目录均可运行）

import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const bannedExtensions = [".py", ".cs", ".js", ".mjs", ".cjs"];
const exemptPrefixes = [".agents/skills/", "third-party/"];

// 以脚本自身位置定位仓库根，cd 进去执行，保证从任意目录可跑
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
process.chdir(repoRoot);

// git ls-files -z：索引全量受管文件，\0 分隔（R2 裁定口径，路径含空格也安全）
const output = execSync("git ls-files -z", { encoding: "buffer" });
const trackedFiles = output.toString("utf8").split("\0").filter(Boolean);

// 允许清单：一行一个仓库相对路径，# 注释与空行忽略；路径规范化为 / 分隔
const allowlistFile = path.join(repoRoot, "scripts", "language-policy-allowlist.txt");
const allowlist = new Set(
  readFileSync(allowlistFile, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim().replaceAll("\\", "/").replace(/^\.\//, ""))
    .filter((line) => line.length > 0 && !line.startsWith("#")),
);

const trackedSet = new Set(trackedFiles);
const violations = [];
let exemptCount = 0; // vendored 豁免
let allowlistCount = 0; // 清单豁免

for (const file of trackedFiles) {
  const lower = file.toLowerCase();
  if (!bannedExtensions.some((ext) => lower.endsWith(ext))) continue;
  if (exemptPrefixes.some((prefix) => file.startsWith(prefix))) {
    exemptCount += 1;
    continue;
  }
  if (allowlist.has(file)) {
    allowlistCount += 1;
    continue;
  }
  violations.push(file);
}

// 陈旧清单项：清单路径不在索引里 → 只告警、不影响退出码（R7 裁定）
let staleCount = 0;
for (const entry of allowlist) {
  if (!trackedSet.has(entry)) {
    staleCount += 1;
    console.warn(`warn: 语言政策清单陈旧条目（文件已删除，建议同步移除）：${entry}`);
  }
}

if (violations.length > 0) {
  for (const file of violations) console.error(`语言政策违规：${file}`);
  console.error(`语言政策检查失败：违规文件 ${violations.length} 个（处置见 docs/specs/language-policy.md）`);
  process.exitCode = 1;
} else {
  const managedCount = exemptCount + allowlistCount;
  console.log(
    `语言政策检查通过：受管文件 ${managedCount} 个（vendored 豁免 ${exemptCount}、清单豁免 ${allowlistCount}、陈旧 ${staleCount}）`,
  );
}
