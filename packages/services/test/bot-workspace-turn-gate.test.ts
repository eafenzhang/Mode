import assert from "node:assert/strict";
import test from "node:test";

// 背景：多用户并发首发同一 workspace 的 bot 回合无准入控制，冷启动尖峰
// 由平台限流与资源竞争兜底（docs/specs/bot-workspace-turn-admission.md）。

const flush = () => new Promise((resolve) => setImmediate(resolve));

test("cap=2：第三个 enter 阻塞，exit 后按 FIFO 顺序放行", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(2);
  await gate.enter("/w1", "t1");
  await gate.enter("/w1", "t2");
  assert.equal(gate.count("/w1"), 2);

  let thirdDone = false;
  let fourthDone = false;
  const third = gate.enter("/w1", "t3").then(() => {
    thirdDone = true;
  });
  const fourth = gate.enter("/w1", "t4").then(() => {
    fourthDone = true;
  });
  await flush();
  assert.equal(thirdDone, false, "cap 满时第三个必须等待");
  assert.equal(fourthDone, false);

  gate.exit("t1");
  await third;
  assert.equal(thirdDone, true);
  await flush();
  assert.equal(fourthDone, false, "FIFO：t3 先占坑，t4 仍等待");
  assert.equal(gate.count("/w1"), 2);

  gate.exit("t2");
  await fourth;
  assert.equal(fourthDone, true);
  assert.equal(gate.count("/w1"), 2);
});

test("不同 workspace 互不影响；enter 幂等；exit 未知/no-op", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(1);
  await gate.enter("/w1", "t1");
  // w2 独立计数，立即放行
  await gate.enter("/w2", "t2");
  assert.equal(gate.count("/w1"), 1);
  assert.equal(gate.count("/w2"), 1);

  // 同 taskId 重复 enter 幂等（不占双份槽位、不阻塞）
  await gate.enter("/w1", "t1");
  assert.equal(gate.count("/w1"), 1);

  // 未知与重复 exit 均为 no-op，不抛错
  gate.exit("unknown");
  gate.exit("t1");
  gate.exit("t1");
  assert.equal(gate.count("/w1"), 0);
});

test("reset 清空跟踪并唤醒全部等待者，唤醒后按当前计数重新竞争", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(1);
  await gate.enter("/w1", "t1");
  let wokenA = false;
  let wokenB = false;
  const a = gate.enter("/w1", "wa").then(() => {
    wokenA = true;
  });
  const b = gate.enter("/w1", "wb").then(() => {
    wokenB = true;
  });
  await flush();
  assert.equal(wokenA, false);

  gate.reset();
  assert.equal(gate.count("/w1"), 0);
  await a;
  await flush();
  // cap=1：a 占坑后 b 仍需等待（唤醒≠无条件放行）
  assert.equal(wokenA, true);
  assert.equal(wokenB, false);
  gate.exit("wa");
  await b;
  assert.equal(wokenB, true);
});
