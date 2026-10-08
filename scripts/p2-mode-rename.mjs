// P2 命名统一的机械替换脚本（配套 docs/specs/p2-mode-naming.md）。
//
// 设计要点：
// - 只做「无歧义的字符串替换」，语义改动（兼容链、迁移逻辑）在源码里手改，脚本不猜；
// - 逐阶段执行（--stage env|packages|artifacts|dataroot|plugins|protocols），--stage all 按顺序跑；
// - 幂等：替换前后同一字符串不存在时记为 0 命中，重复执行是 no-op；
// - 明确的不动清单：开源声明、第三方台账、历史 spec、上游审计 workflow、字体与 VSCode 派生素材。
//
// 用法：
//   node scripts/p2-mode-rename.mjs --stage env --dry
//   node scripts/p2-mode-rename.mjs --stage env
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const argv = process.argv.slice(2);
const stage = argv.includes("--stage") ? argv[argv.indexOf("--stage") + 1] : "all";
const dryRun = argv.includes("--dry");

const TEXT_EXTENSIONS = new Set([
  ".ts", ".tsx", ".js", ".mjs", ".cjs", ".json", ".yaml", ".yml",
  ".md", ".html", ".css", ".sh", ".nsh", ".toml", ".example", ".bash", ".ps1",
]);

/** 目录黑名单：构建产物、依赖、以及必须原样保留的第三方与历史材料。 */
const SKIP_DIRS = new Set([
  "node_modules", ".git", "dist", "dist-types", "out", "mock-cdn", "bundled-agents",
  "build", // packages/desktop/build 内是二进制素材与安装器脚本，单独处理
  "third-party", "license-texts", "ai-elements", "fonts", ".agents", "assets",
  "coverage", ".turbo", "specs", // docs/specs 由人工维护
]);

/** 文件黑名单：开源声明与台账，一律不碰。 */
const SKIP_FILES = new Set([
  "LICENSE", "LICENSE-APACHE", "NOTICE.md", "NOTICE.zh-CN.md", "THIRD-PARTY-NOTICES.md",
  "OFL.txt", "pnpm-lock.yaml",
]);

const SKIP_PATH_PARTS = [
  "scripts/license-texts/",
  "packages/ui/src/components/ai-elements/",
  "packages/ui/src/fonts/",
  ".github/workflows/upstream-audit.yml",
  "docs/specs/p0-user-facing-naming.md",
  "docs/specs/p1a-external-env-renames.md",
];

function walk(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    const rel = relative(root, full).replace(/\\/g, "/");
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      if (SKIP_PATH_PARTS.some((part) => rel.startsWith(part.replace(/\/$/, "")))) continue;
      walk(full, files);
      continue;
    }
    if (!entry.isFile()) continue;
    if (SKIP_FILES.has(entry.name)) continue;
    if (SKIP_PATH_PARTS.some((part) => rel === part.replace(/\/$/, "") || rel.startsWith(part))) continue;
    if (!TEXT_EXTENSIONS.has(extname(entry.name))) continue;
    files.push(full);
  }
  return files;
}

/** 各阶段的替换规则：按顺序应用，前一条的产物会参与后一条匹配。 */
const STAGES = {
  env: [
    // 对外变量：ZCODIUM_* → MODE_*（兼容链在 packages/shared/src/env-names.ts 手改）
    [/ZCODIUM_/g, "MODE_"],
    // 内部进程间变量：ZCODE_* → MODE_*，但保留兼容链里作为兜底的旧名（env-names.ts 与它的测试除外）
    [/ZCODE_/g, "MODE_"],
    [/__ZCODE_/g, "__MODE_"],
    [/ZCODE_APP_VERSION/g, "MODE_APP_VERSION"],
  ],
  packages: [
    [/@mode\//g, "@mode/"],
  ],
  artifacts: [
    [/zcodium-(\$?\{?[A-Za-z0-9_.$}<>-]*)\.tar\.gz/g, "mode-$1.tar.gz"],
    [/dist\/mode\.cjs/g, "dist/mode.cjs"],
    [/mode\.cjs/g, "mode.cjs"],
    [/CLI_COMMAND_NAME = "zcodium"/g, 'CLI_COMMAND_NAME = "mode"'],
    [/"zcodium": "\.\/dist\/mode\.cjs"/g, '"mode": "./dist/mode.cjs"'],
    [/"bin": \{ "zcodium": "\.\/dist\/mode\.cjs" \}/g, '"bin": { "mode": "./dist/mode.cjs" }'],
  ],
  dataroot: [
    [/\.zcodium/g, ".mode"],
    [/ZCodium/g, "Mode"],
  ],
  plugins: [
    [/zcode-plugins-official/g, "mode-plugins-official"],
    [/\.zcode-plugin/g, ".mode-plugin"],
    [/\.zcodeignore/g, ".modeignore"],
  ],
  // 模块与标识符命名：目录、文件名、符号统一到 mode-*（引用与文件名同批替换）
  identifiers: [
    [/mode-protocol/g, "mode-protocol"],
    [/modeAgentService/g, "modeAgentService"],
    [/modeSessionService/g, "modeSessionService"],
    [/ZCodeAgentService/g, "ModeAgentService"],
    [/ZCodeSessionService/g, "ModeSessionService"],
    [/modeEndpoint/g, "modeEndpoint"],
    [/modeUiError/g, "modeUiError"],
    [/mode-agent/g, "mode-agent"],
    [/mode-session/g, "mode-session"],
    [/mode-cua/g, "mode-cua"],
    [/mode-server-cli/g, "mode-server-cli"],
    [/mode-distribution/g, "mode-distribution"],
    [/mode-cli/g, "mode-cli"],
    [/ZCodeBuiltin/g, "ModeBuiltin"],
    [/mode-builtin/g, "mode-builtin"],
    [/modeBuiltin/g, "modeBuiltin"],
    [/Zcode/g, "Mode"],
    [/ZCode/g, "Mode"],
    [/window\.mode\b/g, "window.mode"],
    [/exposeInMainWorld\(\s*\x22zcode\x22/g, "exposeInMainWorld(\x22mode\x22"],
  ],
  protocols: [
    [/"ZCode Protocol"/g, '"Mode Protocol"'],
    [/ZCode Protocol\//g, "Mode Protocol/"],
    [/mode_lite_token/g, "mode_lite_token"],
    [/com\.mode\/official-mcp-auth/g, "com.mode/official-mcp-auth"],
    [/com\.mode\/request-context/g, "com.mode/request-context"],
    [/const DEEP_LINK_SCHEME = "mode"/g, 'const DEEP_LINK_SCHEME = "mode"'],
    [/schemes: \["mode"\]/g, 'schemes: ["mode"]'],
    [/zcodium:\/\//g, "mode://"],
    [/"WECOM_QR_SOURCE = "zcodium"/g, '"WECOM_QR_SOURCE = "mode"'],
    [/appName: "ZCodium"/g, 'appName: "Mode"'],
  ],
};

// 改名脚本自己和新 spec 也在仓库里，必须排除，否则脚本会被自己改写。
const SELF = ["scripts/p2-mode-rename.mjs", "docs/specs/p2-mode-naming.md"];
const EXCLUDED_FROM = {
  env: [
    "packages/shared/src/env-names.ts",
    "packages/shared/test/env-names.test.ts",
    ...SELF,
  ],
  packages: [...SELF],
  identifiers: [...SELF],
  artifacts: [...SELF],
  dataroot: [...SELF],
  plugins: [...SELF],
  protocols: [...SELF],
};

function runStage(name) {
  const rules = STAGES[name];
  if (!rules) throw new Error(`未知阶段：${name}`);
  const skip = new Set(EXCLUDED_FROM[name].map((p) => p.replace(/\\/g, "/")));
  const files = walk(root);
  let touched = 0;
  const hits = new Map();
  for (const file of files) {
    const rel = relative(root, file).replace(/\\/g, "/");
    if (skip.has(rel)) continue;
    let text = readFileSync(file, "utf8");
    const before = text;
    for (const [pattern, replacement] of rules) {
      text = text.replace(pattern, replacement);
    }
    if (text === before) continue;
    touched += 1;
    for (const [pattern] of rules) {
      const count = (before.match(pattern) ?? []).length;
      if (count > 0) hits.set(String(pattern), (hits.get(String(pattern)) ?? 0) + count);
    }
    if (!dryRun) writeFileSync(file, text, "utf8");
  }
  console.log(`\n[stage ${name}] ${dryRun ? "（dry-run）" : ""}改动文件 ${touched} 个`);
  for (const [pattern, count] of [...hits.entries()].sort((a, b) => b[1] - a[1])) {
    console.log(`   ${String(count).padStart(6)}  ${pattern}`);
  }
  return touched;
}

const order = ["env", "packages", "identifiers", "artifacts", "dataroot", "plugins", "protocols"];
for (const name of stage === "all" ? order : [stage]) {
  runStage(name);
}
console.log("\n提醒：语义改动（env-names 兼容链、数据根迁移、插件 id 迁移、协议接受旧值）需手改，见 spec。");
