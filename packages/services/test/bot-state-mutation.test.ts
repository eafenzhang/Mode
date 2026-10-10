import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { BotsStateFile, BotConversationState } from "@mode/shared";
import { setDataBaseDir } from "../src/paths.js";

// 背景：bot-state 曾是 readState→改→writeState 快照覆盖，两次文件锁之间
// 跨对话/跨窗口并发会丢更新（docs/specs/bot-state-ownership.md）。
// 本测试把「锁内原子 RMW」的零丢失承诺固定下来。

async function withTempDataDir(run: (dir: string) => Promise<void>): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), "mode-bot-state-mutation-"));
  setDataBaseDir(dir);
  try {
    await mkdir(join(dir, ".mode", "v2"), { recursive: true });
    await run(dir);
  } finally {
    setDataBaseDir(null);
    await rm(dir, { recursive: true, force: true });
  }
}

function makeConversation(botId: string, id: string): BotConversationState {
  return {
    botId,
    conversationKey: `private:${id}`,
    conversationKind: "private",
    conversationId: id,
    workspacePath: "C:/workspace",
    mode: "draft",
    activeTaskId: null,
    updatedAt: 1,
  };
}

test("并发 20 个 mutateState 各写各的对话槽位：零丢失", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await Promise.all(
      Array.from({ length: 20 }, (_unused, index) =>
        repo.mutateState((state: BotsStateFile) => {
          const channel = (state.bots["bot-1"] ??= {
            botId: "bot-1",
            conversations: {},
            updatedAt: Date.now(),
          });
          channel.conversations[`private:u${index}`] = makeConversation("bot-1", `u${index}`);
          channel.updatedAt = Date.now();
        }),
      ),
    );
    const final = await repo.readState();
    const conversations = final.bots["bot-1"]?.conversations ?? {};
    assert.equal(Object.keys(conversations).length, 20);
  });
});

test("并发 20 次同槽位追加：全部保留", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.mutateState((state: BotsStateFile) => {
      state.bots["bot-1"] = {
        botId: "bot-1",
        conversations: { "private:u1": makeConversation("bot-1", "u1") },
        updatedAt: Date.now(),
      };
    });
    await Promise.all(
      Array.from({ length: 20 }, (_unused, index) =>
        repo.mutateState((state: BotsStateFile) => {
          const slot = state.bots["bot-1"]!.conversations["private:u1"]!;
          slot.queuedMessages = [
            ...(slot.queuedMessages ?? []),
            { text: `m${index}`, receivedAt: index },
          ];
        }),
      ),
    );
    const final = await repo.readState();
    const queued = final.bots["bot-1"]!.conversations["private:u1"]!.queuedMessages ?? [];
    assert.equal(queued.length, 20);
    assert.deepEqual(
      queued.map((item) => item.text).sort(),
      Array.from({ length: 20 }, (_unused, index) => `m${index}`).sort(),
    );
  });
});

test("mutator 抛错或写入非法值：状态不变且错误冒泡", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.mutateState((state: BotsStateFile) => {
      state.bots["bot-1"] = {
        botId: "bot-1",
        conversations: { "private:u1": makeConversation("bot-1", "u1") },
        updatedAt: Date.now(),
      };
    });
    await assert.rejects(() =>
      repo.mutateState((state: BotsStateFile) => {
        state.bots["bot-1"]!.conversations["private:u1"]!.workspacePath = "";
        throw new Error("boom");
      }),
    );
    // 非法值（workspacePath 空串违反 schema）也必须被拦截且不落盘
    await assert.rejects(() =>
      repo.mutateState((state: BotsStateFile) => {
        state.bots["bot-1"]!.conversations["private:u1"]!.workspacePath = "";
      }),
    );
    const final = await repo.readState();
    assert.equal(final.bots["bot-1"]!.conversations["private:u1"]!.workspacePath, "C:/workspace");
  });
});
