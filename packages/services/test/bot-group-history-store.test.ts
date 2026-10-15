import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 群历史落盘（docs/specs/im-bot-group-chat-alignment.md「G7 实现口径」）：
// 独立文件 + 去抖，避免群消息高频入站触发 bot-state 的全量状态写盘；
// 崩溃最多丢一个去抖窗口的追加，这是明确接受的取舍。

async function withTempDir<T>(run: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "bot-gh-"));
  try {
    return await run(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

const snapshotFixture = {
  version: 1 as const,
  bots: { "bot-1": { "chat-A": [{ text: "重启前说过的话", at: 1 }] } },
};

test("load：缺失 / 损坏 / 形状不符都返回 null，不抛错", async () => {
  await withTempDir(async (dir) => {
    const { createGroupHistoryPersistence } = await import("../src/bots/groupHistoryStore.js");
    const missing = createGroupHistoryPersistence({ filePath: join(dir, "none.json") });
    assert.equal(await missing.load(), null);

    const broken = join(dir, "broken.json");
    await import("node:fs/promises").then((fs) => fs.writeFile(broken, "{not json"));
    assert.equal(await createGroupHistoryPersistence({ filePath: broken }).load(), null);

    const wrongShape = join(dir, "wrong.json");
    await import("node:fs/promises").then((fs) =>
      fs.writeFile(wrongShape, JSON.stringify({ version: 1, bots: { "bot-1": { c: [{}] } } })),
    );
    assert.equal(await createGroupHistoryPersistence({ filePath: wrongShape }).load(), null);

    const good = join(dir, "good.json");
    await import("node:fs/promises").then((fs) =>
      fs.writeFile(good, JSON.stringify(snapshotFixture)),
    );
    assert.deepEqual(await createGroupHistoryPersistence({ filePath: good }).load(), snapshotFixture);
  });
});

test("flush：立即落盘；无待写内容时不创建文件", async () => {
  await withTempDir(async (dir) => {
    const { createGroupHistoryPersistence } = await import("../src/bots/groupHistoryStore.js");
    const filePath = join(dir, "history.json");
    const persistence = createGroupHistoryPersistence({ filePath, debounceMs: 60_000 });

    await persistence.flush();
    await assert.rejects(readFile(filePath, "utf8"), "无待写内容不应产生文件");

    persistence.schedule(snapshotFixture);
    await persistence.flush();
    assert.deepEqual(JSON.parse(await readFile(filePath, "utf8")), snapshotFixture);
  });
});

test("去抖：连续 schedule 只落一次，且落的是最新快照", async () => {
  await withTempDir(async (dir) => {
    const { createGroupHistoryPersistence } = await import("../src/bots/groupHistoryStore.js");
    const filePath = join(dir, "history.json");
    const persistence = createGroupHistoryPersistence({ filePath, debounceMs: 20 });

    persistence.schedule({ version: 1, bots: { "bot-1": { "chat-A": [{ text: "第一次", at: 1 }] } } });
    persistence.schedule({
      version: 1,
      bots: { "bot-1": { "chat-A": [{ text: "第一次", at: 1 }, { text: "第二次", at: 2 }] } },
    });

    // 等去抖触发后写盘；轮询避免依赖固定时长。
    const deadline = Date.now() + 2_000;
    let written: unknown;
    for (;;) {
      try {
        written = JSON.parse(await readFile(filePath, "utf8"));
        break;
      } catch {
        if (Date.now() > deadline) {
          throw new Error("去抖窗口内未落盘");
        }
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
    assert.ok(JSON.stringify(written).includes("第二次"), "落盘的必须是最新快照");
    assert.ok(!JSON.stringify(written).includes("第三次"));
    await persistence.dispose();
  });
});

test("源码契约：触发回合读快照（不再取走），append/clear 后排队落盘，且首入站恢复一次", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  assert.ok(
    !/groupHistory\.drain\(/.test(source),
    "旧的 drain（取走即清空）调用必须已全部替换为 snapshot",
  );
  assert.ok(
    source.includes("groupHistory.snapshot(auth.bot.id, message.actor.chatId)"),
    "群聊回合必须用快照读历史",
  );
  const appendSites = source.match(/groupHistory\.append\(/g) ?? [];
  const persistSites = source.match(/persistGroupHistory\(\);/g) ?? [];
  assert.ok(appendSites.length >= 2, "未授权的群消息仍要记历史");
  assert.ok(
    persistSites.length >= appendSites.length + 1,
    "每个 append（以及 clear）之后都要排队落盘",
  );
  assert.ok(
    source.includes("await ensureGroupHistoryLoaded();"),
    "入站入口必须先恢复落盘历史，否则重启后第一批群消息会覆盖成空历史",
  );
  assert.ok(
    !source.includes("groupHistory") || !/bot-group-history[\s\S]{0,80}mutateState/.test(source),
    "群历史不得走 bot-state 的 mutateState",
  );
});
