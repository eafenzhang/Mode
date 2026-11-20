/* oxlint-disable eslint(max-lines) -- 读面/生命周期两组观察共享同一套 marketplace 预置与投影，拆散会复制规则。 */
// C4 Task 0 观察项 3：Plugins 会话面的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-plugins.mts
//
// 预置：本地 marketplace 目录（无网络）经 CLI `plugins marketplace add/install --scope user`
// 写入隔离根；会话面用 plugins/* 方法观察。
// 观察项：list/overview/describe/setEnabled/install/uninstall、marketplace add/remove/update、
// plugins/operationProgress 通知、插件自带 MCP server 的目录接入。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  isolationRecord,
  runOracleCli,
  startC4Harness,
  writeC4Fixture,
  writeLocalMarketplace,
  type C4Harness,
} from "./probe-c4-support.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const SEED = { "hello.txt": "hello\n" };

function oracleCliEnv(dataRoot: string): NodeJS.ProcessEnv {
  return {
    ...process.env,
    HOME: dataRoot,
    USERPROFILE: dataRoot,
    MODE_HOME: dataRoot,
    MODE_DATA_BASE_DIR: dataRoot,
  };
}

/** notifications 投影：plugins/operationProgress 通知序列。 */
function projectProgressNotifications(harness: C4Harness): JsonObject {
  const list = harness.notifications("plugins/operationProgress");
  return { count: list.length, items: list.slice(0, 8) };
}

/** Case 1：读面——无安装的隔离根上 list/overview/describe/referenceCatalog 的形状。 */
async function caseReadFace(): Promise<JsonObject> {
  const harness = await startC4Harness({
    prefix: "c4-plugins-read",
    seed: SEED,
    script: [{ text: "plugins read done" }],
  });
  try {
    const list = await harness.tryRequest("plugins/list", { workspace: harness.workspaceRef() });
    const overview = await harness.tryRequest("plugins/overview", {
      workspace: harness.workspaceRef(),
    });
    const describeMissing = await harness.tryRequest("plugins/describe", {
      workspace: harness.workspaceRef(),
      pluginName: "nonexistent",
      marketplace: "nonexistent-marketplace",
    });
    const describeBundled = await harness.tryRequest("plugins/describe", {
      workspace: harness.workspaceRef(),
      pluginName: "browser-use",
      marketplace: "mode-plugins-official",
    });
    const referenceCatalog = await harness.tryRequest("plugins/referenceCatalog", {
      workspace: harness.workspaceRef(),
    });
    const referenceCatalogWithSession = await harness.tryRequest("plugins/referenceCatalog", {
      workspace: harness.workspaceRef(),
      sessionId: harness.session.sessionId,
    });
    // 投影：bundled 插件的条目形状（id/name/enabled/marketplace/rootPath 折叠）。
    const listResult = (list.result ?? {}) as JsonObject;
    const plugins = Array.isArray(listResult.plugins) ? (listResult.plugins as JsonObject[]) : [];
    return {
      isolation: isolationRecord(harness),
      plugins: plugins.map((plugin) => ({
        id: plugin.id,
        name: plugin.name,
        version: plugin.version,
        enabled: plugin.enabled,
        source: plugin.source,
        marketplace: plugin.marketplace,
        skillCount: plugin.skillCount,
        skillRootCount: plugin.skillRootCount,
        commandRootCount: plugin.commandRootCount,
        components: plugin.components ?? null,
        declaredMcpServerNames: plugin.declaredMcpServerNames ?? null,
        mcpServerNames: plugin.mcpServerNames ?? null,
        rootSource: plugin.rootSource ?? null,
        enabledSource: plugin.enabledSource ?? null,
      })),
      diagnostics: listResult.diagnostics ?? null,
      overview,
      describeMissing,
      describeBundled,
      referenceCatalogEntryNames: Array.isArray(
        (referenceCatalog.result as JsonObject | undefined)?.plugins,
      )
        ? ((referenceCatalog.result as JsonObject).plugins as JsonObject[]).map(
            (entry) => entry.pluginId,
          )
        : null,
      referenceCatalogWithSession:
        (referenceCatalogWithSession.result as JsonObject | undefined)?.authority ?? null,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：生命周期——本地 marketplace 安装后的 list/overview/describe/setEnabled/操作进度。 */
async function caseLifecycle(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-plugins-life-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-plugins-life-ws-"));
  const marketplaceRoot = await mkdtemp(join(tmpdir(), "mode-c4-plugins-market-"));
  const { mcpServerPath } = await writeLocalMarketplace(marketplaceRoot, {
    pluginName: "fixture",
    withMcp: true,
    withSkill: true,
    withCommand: true,
  });
  void mcpServerPath;
  const cliOut: JsonObject[] = [];
  const cli = async (args: string[]): Promise<void> => {
    const output = await runOracleCli(REPO_ROOT, oracleCliEnv(dataRoot), args);
    cliOut.push({ args, output: output.slice(0, 400) });
  };
  await cli(["plugins", "marketplace", "add", marketplaceRoot, "--scope", "user"]);
  await cli(["plugins", "install", "fixture@fixture-marketplace", "--scope", "user"]);

  const harness = await startC4Harness({
    prefix: "c4-plugins-life",
    seed: SEED,
    dataRoot,
    cwd,
    script: [
      { toolCalls: [{ name: "mcp__plugin_fixture_fixture__echo", input: { text: "hello" } }] },
      { text: "plugins lifecycle done" },
    ],
  });
  try {
    // 隔离根之外的临时 marketplace 目录也要折叠成稳定 token（add/overview/CLI 参数都会带它）。
    harness.extras[marketplaceRoot] = "<marketplace-root>";
    const list = await harness.tryRequest("plugins/list", { workspace: harness.workspaceRef() });
    const overview = await harness.tryRequest("plugins/overview", {
      workspace: harness.workspaceRef(),
    });
    const describe = await harness.tryRequest("plugins/describe", {
      workspace: harness.workspaceRef(),
      pluginName: "fixture",
      marketplace: "fixture-marketplace",
    });

    // setEnabled false → true，带 operationId；观察 plugins/operationProgress 通知。
    const disable = await harness.tryRequest("plugins/setEnabled", {
      workspace: harness.workspaceRef(),
      pluginId: "fixture@fixture-marketplace",
      enabled: false,
      operationId: "op-c4-disable",
    });
    const afterDisableNotifications = projectProgressNotifications(harness);
    const listAfterDisable = await harness.tryRequest("plugins/list", {
      workspace: harness.workspaceRef(),
    });
    const enable = await harness.tryRequest("plugins/setEnabled", {
      workspace: harness.workspaceRef(),
      pluginId: "fixture@fixture-marketplace",
      enabled: true,
      operationId: "op-c4-enable",
    });
    const afterEnableNotifications = projectProgressNotifications(harness);

    // 会话面 marketplace 增删改：形状观察（本地目录 source，无网络）。
    // 顺序：update（refresh 已有）→ add（重复添加 → 真实错误形状）→ remove → install 恢复。
    const marketplaceUpdate = await harness.tryRequest("plugins/marketplace/update", {
      workspace: harness.workspaceRef(),
      marketplace: "fixture-marketplace",
    });
    const afterUpdateNotifications = projectProgressNotifications(harness);
    const marketplaceAdd = await harness.tryRequest("plugins/marketplace/add", {
      workspace: harness.workspaceRef(),
      source: marketplaceRoot,
    });
    const afterAddNotifications = projectProgressNotifications(harness);
    const marketplaceRemove = await harness.tryRequest("plugins/marketplace/remove", {
      workspace: harness.workspaceRef(),
      marketplace: "fixture-marketplace",
    });
    const afterRemoveNotifications = projectProgressNotifications(harness);

    // 会话面 install/uninstall：形状观察（remove 后用 install 恢复，继续观察通知）。
    const install = await harness.tryRequest("plugins/install", {
      workspace: harness.workspaceRef(),
      pluginName: "fixture",
      marketplace: "fixture-marketplace",
    });
    const afterInstallNotifications = projectProgressNotifications(harness);
    const uninstall = await harness.tryRequest("plugins/uninstall", {
      workspace: harness.workspaceRef(),
      pluginName: "fixture",
      marketplace: "fixture-marketplace",
    });
    const afterUninstallNotifications = projectProgressNotifications(harness);
    const reinstall = await harness.tryRequest("plugins/install", {
      workspace: harness.workspaceRef(),
      pluginName: "fixture",
      marketplace: "fixture-marketplace",
    });
    const afterReinstallNotifications = projectProgressNotifications(harness);

    // 插件 MCP server 的目录接入（session 面观察，非 sidecar）。
    const turnStart = harness.eventCount();
    await harness.session.send("call the plugin mcp echo");
    await harness.waitForEvent(
      turnStart,
      (event) => event.type === "turn.completed" || event.type === "turn.failed",
      "plugin mcp turn terminal",
      90_000,
    );
    const toolNames = (
      Array.isArray(harness.rawBodies.at(-1)?.tools)
        ? ((harness.rawBodies.at(-1) as JsonObject).tools as JsonObject[])
        : []
    ).map((tool) => String(tool.name ?? ""));
    const events = harness.eventsSince(turnStart);
    return {
      isolation: isolationRecord(harness),
      fixtureListEntry: (Array.isArray((list.result as JsonObject | undefined)?.plugins)
        ? ((list.result as JsonObject).plugins as JsonObject[])
        : []
      )
        .filter((plugin) => plugin.name === "fixture")
        .map((plugin) => ({
          id: plugin.id,
          version: plugin.version,
          enabled: plugin.enabled,
          scope: plugin.rootSource ?? null,
          declaredMcpServerNames: plugin.declaredMcpServerNames ?? null,
          mcpServerNames: plugin.mcpServerNames ?? null,
          skillCount: plugin.skillCount ?? null,
          commandRootCount: plugin.commandRootCount ?? null,
          components: plugin.components ?? null,
        })),
      overviewFixture: overview,
      describe,
      disable,
      enable,
      listAfterDisableFixtureEnabled: (
        ((listAfterDisable.result as JsonObject | undefined)?.plugins as
          | JsonObject[]
          | undefined) ?? []
      )
        .filter((plugin) => plugin.name === "fixture")
        .map((plugin) => plugin.enabled),
      afterDisableNotifications,
      afterEnableNotifications,
      marketplaceUpdate,
      afterUpdateNotifications,
      marketplaceAdd,
      afterAddNotifications,
      marketplaceRemove,
      afterRemoveNotifications,
      install,
      afterInstallNotifications,
      uninstall,
      afterUninstallNotifications,
      reinstall,
      afterReinstallNotifications,
      declaredToolsAfterTurn: toolNames.filter((name) => name.startsWith("mcp__")),
      pluginMcpEvents: events.filter((event) => event.type === "tool.updated"),
      cli: harness.normalizeForFixture(cliOut),
    };
  } finally {
    await harness.stop();
    await rm(marketplaceRoot, { recursive: true, force: true });
  }
}

const fixtures: Record<string, JsonObject> = {
  "plugins-read-face": {
    case: "plugins-read-face",
    note:
      "C4 Task 0 oracle 捕获：无安装隔离根上的 plugins/list（bundled 官方插件条目形状）、" +
      "plugins/overview（marketplaces 空表 + availablePlugins 目录）、describe 的 bundled 命中与" +
      "未知插件错误、plugins/referenceCatalog（workspace/session authority）。",
    input: { seed: SEED, noMarketplace: true },
    expect: await caseReadFace(),
  },
  "plugins-lifecycle": {
    case: "plugins-lifecycle",
    note:
      "C4 Task 0 oracle 捕获：本地 marketplace 经 CLI 安装后，会话面的 list/overview/describe、" +
      "setEnabled(false/true) 的返回与 plugins/operationProgress 通知序列、marketplace add/remove/" +
      "update 与 install/uninstall 的参数接受形状、插件自带 MCP server 进入工具目录并完成一次调用。",
    input: {
      marketplace: {
        name: "fixture-marketplace",
        plugin: "fixture",
        components: ["mcp", "skill", "command"],
      },
      cli: [
        "plugins marketplace add <local> --scope user",
        "plugins install fixture@fixture-marketplace --scope user",
      ],
    },
    expect: await caseLifecycle(),
  },
};

const targets: string[] = [];
for (const [name, fixture] of Object.entries(fixtures)) {
  targets.push(await writeC4Fixture(name, fixture));
}
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      summaries: Object.fromEntries(
        Object.entries(fixtures).map(([name, fixture]) => {
          const expect = fixture.expect as JsonObject;
          return [
            name,
            {
              pluginCount: Array.isArray(expect.plugins) ? expect.plugins.length : null,
              fixtureEntry: Array.isArray(expect.fixtureListEntry) ? expect.fixtureListEntry : null,
              progressNotifications: expect.afterEnableNotifications ?? null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
