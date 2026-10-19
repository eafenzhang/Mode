import assert from "node:assert/strict";
import test from "node:test";

// 背景：A+ 取消 workspace 级 bot 回合上限 2，改为单实例全局有界并发；
// 同一会话串行仍由 enqueueInboundProcessing 保证
// （docs/specs/bot-workspace-turn-admission.md）。

const flush = () => new Promise((resolve) => setImmediate(resolve));

test("全局 cap=2：第三个 enter（任意 workspace）阻塞，exit 后按全局 FIFO 放行", async () => {
  const { createBotTurnGate } = await import("../src/bots/botTurnGate.js");
  const gate = createBotTurnGate(2);
  await gate.enter("/w1", "t1");
  await gate.enter("/w2", "t2");
  assert.equal(gate.count(), 2);

  let thirdDone = false;
  let fourthDone = false;
  const third = gate.enter("/w3", "t3").then(() => {
    thirdDone = true;
  });
  const fourth = gate.enter("/w4", "t4").then(() => {
    fourthDone = true;
  });
  await flush();
  assert.equal(thirdDone, false, "全局 cap 满时第三个必须等待");
  assert.equal(fourthDone, false);

  gate.exit("t1");
  await third;
  assert.equal(thirdDone, true);
  await flush();
  assert.equal(fourthDone, false, "全局 FIFO：t3 先占坑，t4 仍等待");
  assert.equal(gate.count(), 2);

  gate.exit("t2");
  await fourth;
  assert.equal(fourthDone, true);
  assert.equal(gate.count(), 2);
});

test("同一 workspace 不再受上限 2 限制；enter 幂等；exit 未知/no-op", async () => {
  const { createBotTurnGate } = await import("../src/bots/botTurnGate.js");
  const gate = createBotTurnGate(8);
  // 同一 workspace 的三个不同对话在全局槽位内并行，不再被 workspace 级上限卡住。
  await Promise.all([
    gate.enter("/w1", "t1"),
    gate.enter("/w1", "t2"),
    gate.enter("/w1", "t3"),
  ]);
  assert.equal(gate.count("/w1"), 3);
  assert.equal(gate.count(), 3);

  // 同 taskId 重复 enter 幂等（不占双份槽位、不阻塞）
  await gate.enter("/w1", "t1");
  assert.equal(gate.count("/w1"), 3);
  assert.equal(gate.count(), 3);

  // 未知与重复 exit 均为 no-op，不抛错
  gate.exit("unknown");
  assert.equal(gate.count(), 3);
  gate.exit("t1");
  gate.exit("t1");
  assert.equal(gate.count("/w1"), 2);
  assert.equal(gate.count(), 2);
});

test("reset 清空跟踪并唤醒全部等待者，唤醒后按当前全局计数重新竞争", async () => {
  const { createBotTurnGate } = await import("../src/bots/botTurnGate.js");
  const gate = createBotTurnGate(1);
  await gate.enter("/w1", "t1");
  let wokenA = false;
  let wokenB = false;
  const a = gate.enter("/w2", "wa").then(() => {
    wokenA = true;
  });
  const b = gate.enter("/w3", "wb").then(() => {
    wokenB = true;
  });
  await flush();
  assert.equal(wokenA, false);

  gate.reset();
  assert.equal(gate.count(), 0);
  await a;
  await flush();
  // cap=1：a 占坑后 b 仍需等待（唤醒≠无条件放行）
  assert.equal(wokenA, true);
  assert.equal(wokenB, false);
  gate.exit("wa");
  await b;
  assert.equal(wokenB, true);
});
