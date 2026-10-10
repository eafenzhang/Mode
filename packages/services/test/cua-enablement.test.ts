import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 电脑控制启用权威（docs/specs/computer-use-enablement.md）：
// resolveCuaWorkspaceEnablement = 打包层 env kill-switch && 插件启用态（用户权威门）。
// 每例独立 HOME 与 workingDirectory，隔离 user 层 config 与 project 层发现路径，
// 避免测试机真实 ~/.mode/cli/config.json 或仓库内 mode.json 污染判定。
const { resolveCuaWorkspaceEnablement } = await import("../src/cuaEnablement.js");

const CUA_PLUGIN_ID = "computer-use@mode-plugins-official";

interface FixtureOptions {
  pluginEnabled?: boolean;
  suppressed?: boolean;
}

async function makeFixture(options: FixtureOptions = {}) {
  const home = await mkdtemp(join(tmpdir(), "mode-cua-home-"));
  const workingDirectory = await mkdtemp(join(tmpdir(), "mode-cua-ws-"));
  if (options.pluginEnabled !== undefined || options.suppressed) {
    await mkdir(join(home, ".mode", "cli"), { recursive: true });
    await writeFile(
      join(home, ".mode", "cli", "config.json"),
      JSON.stringify({
        plugins: {
          ...(options.pluginEnabled !== undefined
            ? { enabledPlugins: { [CUA_PLUGIN_ID]: options.pluginEnabled } }
            : {}),
          ...(options.suppressed ? { suppressedBuiltins: [CUA_PLUGIN_ID] } : {}),
        },
      }),
      "utf8",
    );
  }
  return { home, workingDirectory };
}

test("插件启用 + env 默认 → 启用", async () => {
  const { home, workingDirectory } = await makeFixture({ pluginEnabled: true });
  assert.equal(resolveCuaWorkspaceEnablement({ env: { HOME: home }, workingDirectory }), true);
});

test("env 默认 + 无配置 → 关闭（默认关不回退为开）", async () => {
  const { home, workingDirectory } = await makeFixture();
  assert.equal(resolveCuaWorkspaceEnablement({ env: { HOME: home }, workingDirectory }), false);
});

test("env 默认 + 显式关闭 → 关闭", async () => {
  const { home, workingDirectory } = await makeFixture({ pluginEnabled: false });
  assert.equal(resolveCuaWorkspaceEnablement({ env: { HOME: home }, workingDirectory }), false);
});

test("打包层 kill-switch 关闭时压过启用配置（stale config 不得越过）", async () => {
  const { home, workingDirectory } = await makeFixture({ pluginEnabled: true });
  assert.equal(
    resolveCuaWorkspaceEnablement({
      env: { HOME: home, MODE_CUA_PRODUCT_HELPER: "0" },
      workingDirectory,
    }),
    false,
  );
});

test("MODE_CUA_DEV_MODE=1 不再绕过插件启用态", async () => {
  const { home, workingDirectory } = await makeFixture();
  assert.equal(
    resolveCuaWorkspaceEnablement({
      env: { HOME: home, MODE_CUA_DEV_MODE: "1" },
      workingDirectory,
    }),
    false,
  );
});

test("suppressedBuiltins 压住启用配置", async () => {
  const { home, workingDirectory } = await makeFixture({
    pluginEnabled: true,
    suppressed: true,
  });
  assert.equal(resolveCuaWorkspaceEnablement({ env: { HOME: home }, workingDirectory }), false);
});
