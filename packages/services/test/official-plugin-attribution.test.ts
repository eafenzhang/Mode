import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// 自研插件署名（docs/specs/first-party-plugin-attribution.md）：商店「开发者」与随包
// manifest 的 author 一律署名 Mode，不回流上游 Z.ai。以仓内文件文本为断言面。
const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
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
// 三个随包 producer 包：listing 缺失时详情页用 manifest author 兜底，必须与 seed 同步。
const manifestPaths = [
  "apps/mode-cli/packages/browser-use-plugin/.mode-plugin/plugin.json",
  "apps/mode-cli/packages/node-repl-host/.mode-plugin/plugin.json",
  "apps/mode-cli/packages/mode-cua-plugin/.mode-plugin/plugin.json",
] as const;

test("官方市场 seed 只用 MODE_AUTHOR 声明开发者，值为 Mode", async () => {
  const source = await readFile(definitionsPath, "utf8");
  assert.match(
    source,
    /const MODE_AUTHOR = \{ name: "Mode" \} as const;/,
    "开发者署名唯一事实源应为 MODE_AUTHOR = { name: \"Mode\" }",
  );
  assert.doesNotMatch(
    source,
    /ZAI_AUTHOR|name: "Z\.ai"/,
    "definitions 不应回流上游 Z.ai 署名（含散落硬编码）",
  );
  // 每条带 listing 的 definition 都必须引用同一个常量，禁止逐条改写署名。
  const authorLines = source.split("\n").filter((line) => /author:/.test(line));
  assert.ok(authorLines.length > 0, "definitions 应存在 listing author 声明");
  assert.ok(
    authorLines.every((line) => /author: MODE_AUTHOR,/.test(line)),
    `listing author 必须全部引用 MODE_AUTHOR，实际：${authorLines.join(" | ")}`,
  );
});

test("随包插件 manifest 的 author 署名 Mode，不含 Z.ai", async () => {
  for (const relativePath of manifestPaths) {
    const raw = await readFile(join(repoRoot, relativePath), "utf8");
    const manifest = JSON.parse(raw) as { author?: { name?: string } };
    assert.equal(
      manifest.author?.name,
      "Mode",
      `${relativePath} 的 author.name 必须是 Mode`,
    );
    assert.doesNotMatch(raw, /Z\.ai/, `${relativePath} 不应出现上游 Z.ai`);
  }
});
