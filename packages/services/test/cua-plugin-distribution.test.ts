import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// 电脑控制插件分发契约（docs/specs/computer-use-plugin-distribution.md）：
// producer 包布局、种子文件与 definitions 声明的机械对照、docs 14 方法覆盖、
// staging 清单接线。全部以仓内文件文本为断言面，不执行打包脚本。
const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
const pluginRoot = join(repoRoot, "apps", "mode-cli", "packages", "mode-cua-plugin");
const definitionsPath = join(
  repoRoot,
  "apps",
  "mode-cli",
  "packages",
  "bootstrap",
  "src",
  "app",
  "official-plugin-definitions.ts",
);
const desktopStagePath = join(
  repoRoot,
  "packages",
  "desktop",
  "scripts",
  "prepare-agent-node-bundle.mjs",
);
const remoteStagePath = join(repoRoot, "scripts", "prepare-prebuilds.mjs");

// 与 packages/mode-cua 的 TOOL_NAMES 及 computer-use-windows-runtime.md 工具面节一致。
const TOOL_NAMES = [
  "list_apps",
  "list_windows",
  "get_app_state",
  "left_click",
  "scroll",
  "left_click_drag",
  "type",
  "set_value",
  "select_text",
  "key",
  "perform_action",
  "paste",
  "request_access",
  "stop_computer_control",
] as const;

// 镜像 OFFICIAL_CUA_REQUIRED_SEED_PATHS；definitions 文本断言防止两处漂移。
const REQUIRED_SEED_PATHS = [
  "docs/computer-use.md",
  "scripts/computer-use-client.mjs",
  "skills/computer-use/SKILL.md",
] as const;

async function readText(path: string): Promise<string> {
  return readFile(path, "utf8");
}

test("producer 包清单：name/version 对齐定义且不声明 mcpServers", async () => {
  const manifest = JSON.parse(await readText(join(pluginRoot, ".mode-plugin", "plugin.json")));
  assert.equal(manifest.name, "computer-use");
  assert.equal(
    "mcpServers" in manifest,
    false,
    "mode-cua 是 skill/SDK-only：宿主不为它生成独立 CUA MCP server",
  );

  const definitions = await readText(definitionsPath);
  const entryStart = definitions.indexOf('name: "computer-use"');
  assert.notEqual(entryStart, -1, "official-plugin-definitions 应包含 computer-use 定义");
  const versionMatch = definitions.slice(entryStart).match(/version: "([0-9]+\.[0-9]+\.[0-9]+)"/);
  assert.ok(versionMatch, "computer-use 定义应带版本号");
  assert.equal(
    manifest.version,
    versionMatch[1],
    "manifest 版本必须对齐官方定义版本，否则市场比较会永久报可更新",
  );

  const pkg = JSON.parse(await readText(join(pluginRoot, "package.json")));
  assert.equal(pkg.name, "@mode/cua-plugin");
});

test("三个必需种子文件存在，且 definitions 的 requiredSeedPaths 与之一致", async () => {
  const definitions = await readText(definitionsPath);
  for (const seedPath of REQUIRED_SEED_PATHS) {
    assert.ok(
      definitions.includes(`"${seedPath}"`),
      `official-plugin-definitions 应声明 requiredSeedPaths "${seedPath}"`,
    );
    const content = await readText(join(pluginRoot, ...seedPath.split("/")));
    assert.ok(content.trim().length > 0, `种子文件 ${seedPath} 不应为空`);
  }
});

test("docs/computer-use.md 以反引号覆盖全部 14 个方法名", async () => {
  const docs = await readText(join(pluginRoot, "docs", "computer-use.md"));
  for (const name of TOOL_NAMES) {
    assert.ok(docs.includes(`\`${name}\``), `docs 缺少方法 \`${name}\``);
  }
});

test("SKILL frontmatter name=computer-use 且正文给出观察入口", async () => {
  const skill = await readText(join(pluginRoot, "skills", "computer-use", "SKILL.md"));
  const frontmatter = skill.match(/^---\n([\s\S]*?)\n---/);
  assert.ok(frontmatter, "SKILL 必须有 frontmatter");
  assert.match(frontmatter[1], /^name: computer-use$/m);
  assert.match(frontmatter[1], /^description: .+$/m);
  assert.match(frontmatter[1], /^when_to_use: .+$/m);
  assert.ok(skill.includes("get_app_state"), "SKILL 正文应指向观察入口 get_app_state");
  assert.ok(
    skill.includes("mode.node-repl.computer-use-bridge"),
    "SKILL 应给出 bridge global 访问方式",
  );
});

test("computer-use-client.mjs 导出 14 方法名常量", async () => {
  const client = await readText(join(pluginRoot, "scripts", "computer-use-client.mjs"));
  assert.ok(client.includes("COMPUTER_METHOD_NAMES"), "SDK 应导出 COMPUTER_METHOD_NAMES");
  for (const name of TOOL_NAMES) {
    const quoted = new RegExp(`['"]${name}['"]`);
    assert.ok(quoted.test(client), `SDK 缺少方法名 '${name}'`);
  }
});

test("staging 清单接线：桌面与远端都 stage mode-cua-plugin", async () => {
  const desktopStage = await readText(desktopStagePath);
  assert.ok(
    desktopStage.includes('relativePath: "apps/mode-cli/packages/mode-cua-plugin"'),
    "桌面清单应含 producer relativePath",
  );
  assert.ok(
    desktopStage.includes('stagedPath: "packages/mode-cua-plugin"'),
    "桌面清单应含 stagedPath（rootCandidates 打包态解析目标）",
  );

  const remoteStage = await readText(remoteStagePath);
  assert.ok(
    remoteStage.includes('relativePath: "apps/mode-cli/packages/mode-cua-plugin"'),
    "远端清单应含 producer relativePath",
  );
  assert.ok(
    remoteStage.includes('stagedPath: "packages/mode-cua-plugin"'),
    "远端清单应含 stagedPath",
  );
  assert.ok(
    remoteStage.includes("packages/mode-cua-plugin/.mode-plugin/plugin.json"),
    "远端 requiredPaths 应校验插件 manifest 存在",
  );
});
