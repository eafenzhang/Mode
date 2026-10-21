import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test, { after } from "node:test";
import type { BotsConfigFile } from "@mode/shared";

// 事故依据（2026-10-21）：MODE_DATA_BASE_DIR 已由宿主环境固定时，setDataBaseDir()
// 按设计是 no-op；旧写法仍会继续写真实 ~/.mode/v2，测试夹具曾覆盖开发者 bot-config。
// 现在必须在任何 services 模块加载前设置环境变量，并用 getAppConfigDir() 复核隔离。
const previousDataBaseDir = process.env.MODE_DATA_BASE_DIR;
const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-config-mutation-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
mkdirSync(join(dataDir, ".mode", "v2"), { recursive: true });

after(() => {
  if (previousDataBaseDir === undefined) delete process.env.MODE_DATA_BASE_DIR;
  else process.env.MODE_DATA_BASE_DIR = previousDataBaseDir;
  rmSync(dataDir, { recursive: true, force: true });
});

async function withTempDataDir(run: () => Promise<void>): Promise<void> {
  const { getAppConfigDir } = await import("../src/paths.js");
  assert.ok(
    getAppConfigDir().startsWith(dataDir),
    `测试数据目录隔离失败：${getAppConfigDir()} 不在 ${dataDir} 下，拒绝写入真实数据目录`,
  );
  await run();
}

function config(): BotsConfigFile {
  return {
    version: 3,
    bots: [
      {
        id: "bot-1",
        name: "bot",
        provider: "wecom",
        enabled: true,
        allowedWorkspaces: ["*"],
        allowedCommands: {
          status: true,
          new: true,
          workspace: true,
          model: true,
          mode: true,
          thoughtLevel: true,
          reply: true,
        },
        currentOptions: {},
        replyMode: "streaming_card",
      },
    ],
  };
}

test("mutateConfig：并发 20 次群授权追加零丢失", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.writeConfig(config());
    await Promise.all(
      Array.from({ length: 20 }, (_unused, index) =>
        repo.mutateConfig((next) => {
          const bot = next.bots[0]!;
          const permissions = bot.groupChat?.permissions ?? [];
          bot.groupChat = {
            activation: bot.groupChat?.activation ?? "mention",
            ...bot.groupChat,
            permissions: [
              ...permissions,
              {
                groupId: `group-${index}`,
                groupName: `group-${index}`,
                platform: bot.provider,
                status: "pending",
                discoveredAt: index,
              },
            ],
          };
        }),
      ),
    );
    const saved = await repo.readConfig();
    const permissions = saved.bots[0]!.groupChat?.permissions ?? [];
    assert.equal(permissions.length, 20);
  });
});

test("mutateConfig：mutator 抛错时不落盘", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.writeConfig(config());
    await assert.rejects(() =>
      repo.mutateConfig((next) => {
        next.bots[0]!.name = "changed";
        throw new Error("boom");
      }),
    );
    const saved = await repo.readConfig();
    assert.equal(saved.bots[0]!.name, "bot");
  });
});
