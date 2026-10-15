import assert from "node:assert/strict";
import test from "node:test";

// 变更摘要里的「新增 / 修改 / 删除」（docs/specs/bot-reply-granularity.md 的变更摘要构成、
// 以及 bot 侧变更类型推导）：
// 原来只报文件数 + 行数增删，新建的文件和改过的文件在 IM 回复里长得一样。
// 判定不新增共享协议字段——bot 已持有原始快照，本地推导即可。

function snapshot(path: string, before: string | null, after: string) {
  return { path, beforeContent: before, afterContent: after, writeCount: 1 };
}

test("按原始快照推导：beforeContent 为 null 是新增、最终为空是删除、其余是修改", async () => {
  const { deriveBotFileChangeKinds } = await import("../src/bots/botChangeSummary.js");
  const kinds = deriveBotFileChangeKinds(
    [
      {
        turnIndex: 3,
        snapshots: [
          snapshot("src/new.ts", null, "export const a = 1;\n"),
          snapshot("src/old.ts", "const b = 1;\n", "const b = 2;\n"),
          snapshot("src/gone.ts", "临时文件\n", ""),
        ],
      },
    ],
    3,
  );
  assert.equal(kinds.get("src/new.ts"), "added");
  assert.equal(kinds.get("src/old.ts"), "modified");
  assert.equal(kinds.get("src/gone.ts"), "deleted");
});

test("同一文件同轮多次写入：按首个 before 与末个 after 判定", async () => {
  const { deriveBotFileChangeKinds } = await import("../src/bots/botChangeSummary.js");
  // 新建后再改两次：仍是"新增"（首个 beforeContent 为 null）
  const created = deriveBotFileChangeKinds(
    [
      {
        turnIndex: 1,
        snapshots: [
          snapshot("a.ts", null, "1\n"),
          snapshot("a.ts", "1\n", "1\n2\n"),
          snapshot("a.ts", "1\n2\n", "1\n2\n3\n"),
        ],
      },
    ],
    1,
  );
  assert.equal(created.get("a.ts"), "added");

  // 原本存在、最终被清空：是"删除"
  const emptied = deriveBotFileChangeKinds(
    [
      {
        turnIndex: 1,
        snapshots: [snapshot("b.ts", "x\n", "y\n"), snapshot("b.ts", "y\n", "")],
      },
    ],
    1,
  );
  assert.equal(emptied.get("b.ts"), "deleted");
});

test("只取目标轮次；轮次缺失或fileChanges 为空时返回空映射", async () => {
  const { deriveBotFileChangeKinds } = await import("../src/bots/botChangeSummary.js");
  const changes = [
    { turnIndex: 1, snapshots: [snapshot("t1.ts", null, "x\n")] },
    { turnIndex: 2, snapshots: [snapshot("t2.ts", "x\n", "y\n")] },
  ];
  const second = deriveBotFileChangeKinds(changes, 2);
  assert.equal(second.has("t2.ts"), true);
  assert.equal(second.has("t1.ts"), false, "历史轮次不得混进本轮摘要");
  assert.equal(deriveBotFileChangeKinds(changes, 9).size, 0);
  assert.equal(deriveBotFileChangeKinds(undefined, 1).size, 0);
});

test("渲染：带变更类型时每行带类型标签与路径；无类型时退回原格式", async () => {
  const { formatBotAssistantReplyBlocks } = await import("../src/bots/replyFormatter.js");
  const changeSummary = {
    fileCount: 2,
    added: 4,
    removed: 1,
    files: [
      { path: "src/new.ts", added: 3, removed: 0, writeCount: 1, lastTurnIndex: 1 },
      { path: "src/old.ts", added: 1, removed: 1, writeCount: 2, lastTurnIndex: 1 },
    ],
  };

  const withKinds = formatBotAssistantReplyBlocks(
    [{ type: "change-summary", changeSummary }],
    { locale: "zh-CN", changeKinds: new Map([["src/new.ts", "added" as const]]) },
  );
  assert.equal(withKinds.length, 1);
  assert.ok(withKinds[0]!.includes("新增 `src/new.ts`"), "新增文件必须标出类型");
  assert.ok(withKinds[0]!.includes("`src/old.ts`"), "缺类型的文件仍要出现");

  const withoutKinds = formatBotAssistantReplyBlocks(
    [{ type: "change-summary", changeSummary }],
    { locale: "zh-CN" },
  );
  assert.ok(!withoutKinds[0]!.includes("新增"));

  const en = formatBotAssistantReplyBlocks(
    [{ type: "change-summary", changeSummary }],
    { locale: "en-US", changeKinds: new Map([["src/new.ts", "added" as const]]) },
  );
  assert.ok(en[0]!.includes("Added `src/new.ts`"), "英文环境用 Added/Modified/Deleted");
});
