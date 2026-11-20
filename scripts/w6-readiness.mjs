/**
 * W6 收口就绪审计（LT-11）：只读扫描产品代码里仍存在的「Node Host / stdio 帧」依赖。
 *
 * W6 的删除动作要等 W5 剩余生态面完成；本脚本不执行删除，只把阻塞项按
 * 文件:行号列出来，避免「以为只剩一处」的误判。默认永远 exit 0（报告模式）；
 * `--fail-on-blockers` 供真正宣布收口时做门禁。
 *
 * 用法：
 *   pnpm w6:readiness
 *   pnpm w6:readiness --json
 *   pnpm w6:readiness --fail-on-blockers
 */
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = process.cwd();
const SCAN_ROOTS = ["packages", "crates", "apps"];
const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  "dist",
  "dist-release",
  ".git",
  ".plan",
  "build",
  "coverage",
]);
const SOURCE_EXTENSIONS = [".ts", ".tsx", ".mts", ".mjs", ".js", ".rs", ".json", ".toml", ".yml", ".yaml"];

const BLOCKERS = [
  { id: "stdioFrames", pattern: /\bstdioFrames\b/g, label: "stdio 帧传输仍在产品代码" },
  { id: "stdoutGuard", pattern: /\bstdoutGuard\b/g, label: "stdout 守卫仍在产品代码" },
  { id: "host-sidecar", pattern: /host-sidecar/g, label: "host-sidecar 发运集/脚本仍存在" },
  { id: "node-agent-fallback", pattern: /\bbundledAgentRuntime\b|\bmode\.cjs\b/g, label: "Node agent 回退仍随包" },
];

function walk(directory, files) {
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      walk(join(directory, entry.name), files);
      continue;
    }
    if (!SOURCE_EXTENSIONS.some((extension) => entry.name.endsWith(extension))) continue;
    files.push(join(directory, entry.name));
  }
}

const files = [];
for (const root of SCAN_ROOTS) walk(join(ROOT, root), files);

const findings = new Map(BLOCKERS.map((blocker) => [blocker.id, []]));
for (const file of files) {
  const text = readFileSync(file, "utf8");
  if (text.length > 4 * 1024 * 1024) continue;
  const relativePath = relative(ROOT, file).split("\\").join("/");
  const isTest = /(^|\/)(test|tests|__tests__)(\/|$)|\.(test|spec)\./.test(relativePath);
  for (const blocker of BLOCKERS) {
    blocker.pattern.lastIndex = 0;
    let match;
    while ((match = blocker.pattern.exec(text)) !== null) {
      const line = text.slice(0, match.index).split("\n").length;
      findings.get(blocker.id).push({ file: relativePath, line, isTest });
    }
  }
}

const report = {
  generatedAt: new Date().toISOString(),
  scannedFiles: files.length,
  blockers: BLOCKERS.map((blocker) => {
    const hits = findings.get(blocker.id);
    const product = hits.filter((hit) => !hit.isTest);
    return {
      id: blocker.id,
      label: blocker.label,
      total: hits.length,
      product: product.length,
      tests: hits.length - product.length,
      files: [...new Set(product.map((hit) => hit.file))].sort(),
      sample: product.slice(0, 12),
    };
  }),
};
report.ready = report.blockers.every((blocker) => blocker.product === 0);

if (process.argv.includes("--json")) {
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} else {
  process.stdout.write(`W6 readiness（扫描 ${report.scannedFiles} 个文件）\n`);
  for (const blocker of report.blockers) {
    process.stdout.write(
      `- ${blocker.id}: product=${blocker.product} tests=${blocker.tests} ` +
        `files=${blocker.files.length} — ${blocker.label}\n`,
    );
    for (const hit of blocker.sample) {
      process.stdout.write(`    ${hit.file}:${hit.line}\n`);
    }
  }
  process.stdout.write(`结论：${report.ready ? "READY" : "NOT READY"}\n`);
}

if (!report.ready && process.argv.includes("--fail-on-blockers")) {
  process.exitCode = 1;
}
