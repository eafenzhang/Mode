import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { BotsConfigFile } from "@mode/shared";
import { setDataBaseDir } from "../src/paths.js";

async function withTempDataDir(run: () => Promise<void>): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), "mode-bot-config-mutation-"));
  setDataBaseDir(dir);
  try {
    await mkdir(join(dir, ".mode", "v2"), { recursive: true });
    await run();
  } finally {
    setDataBaseDir(null);
    await rm(dir, { recursive: true, force: true });
  }
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
