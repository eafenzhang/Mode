import assert from "node:assert/strict";
import test from "node:test";

// 整会话重连控制器（P2-1）：重试分类（busy/internal 重试、明确拒绝立即收口）、
// 成功接力新 mediaUrl、缺 mediaUrl 按不支持收口、dispose 作废在途与定时器。
const { createSessionReconnectController } = await import(
  "../src/remote-desktop/sessionReconnectController.js"
);

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** 便捷构造：记录起点/动作，返回控制器与收集器。 */
function harness(
  startSession: () => Promise<{ ok: true; mediaUrl?: string } | { error: string }>,
  maxWaitMs = 1_600,
) {
  const collector = {
    calls: 0,
    begins: 0,
    recovered: [] as string[],
    gaveUp: [] as string[],
  };
  const controller = createSessionReconnectController({
    startSession: async () => {
      collector.calls += 1;
      return (await startSession()) as never;
    },
    onBegin: () => {
      collector.begins += 1;
    },
    onRecovered: (url) => collector.recovered.push(url),
    onGiveUp: (error) => collector.gaveUp.push(error),
  });
  return { controller, collector, settle: () => sleep(maxWaitMs) };
}

test("busy/internal 逐档重试直到成功；onBegin 只触发一次（幂等）", async () => {
  const { controller, collector, settle } = harness(async () => {
    if (collector.calls <= 1) return { error: "busy" } as never;
    if (collector.calls === 2) return { error: "internal" } as never;
    return { ok: true, mediaUrl: "ws://peer/media?mediaTicket=t" } as never;
  });
  controller.begin();
  controller.begin(); // 退避进行中重复 begin：忽略（不叠加循环）
  await settle();
  assert.equal(collector.begins, 1, "onBegin 幂等");
  assert.equal(collector.calls, 3, "busy→internal→成功共 3 次尝试");
  assert.deepEqual(collector.recovered, ["ws://peer/media?mediaTicket=t"]);
  assert.deepEqual(collector.gaveUp, [], "成功路径不触发 giveUp");
});

test("明确拒绝（consent_denied）立即收口，不重试", async () => {
  const { controller, collector, settle } = harness(async () => ({ error: "consent_denied" }) as never);
  controller.begin();
  await settle();
  assert.equal(collector.calls, 1, "只尝试一次");
  assert.deepEqual(collector.gaveUp, ["consent_denied"]);
  assert.deepEqual(collector.recovered, []);
});

test("成功但缺 mediaUrl：按 unsupported 收口（契约 E 与首连同口径）", async () => {
  const { controller, collector, settle } = harness(async () => ({ ok: true }) as never);
  controller.begin();
  await settle();
  assert.deepEqual(collector.gaveUp, ["unsupported"]);
  assert.deepEqual(collector.recovered, []);
});

test("dispose 停表：退避期未触发即作废；在途结果被丢弃", async () => {
  // 情形一：dispose 早于首个退避到点 → 零尝试。
  const early = harness(async () => ({ error: "busy" }) as never, 600);
  early.controller.begin();
  early.controller.dispose();
  await early.settle();
  assert.equal(early.collector.calls, 0, "定时器被清除，不再尝试");

  // 情形二：在途 startSession 悬挂中 dispose → 结果到达不再回调。
  let release: ((value: unknown) => void) | null = null;
  const pending = new Promise((resolve) => {
    release = resolve;
  });
  const inflight = harness(() => pending as never, 400);
  inflight.controller.begin();
  await sleep(320); // 首个退避（250ms）到点，进入在途
  assert.equal(inflight.collector.calls, 1, "已进入在途尝试");
  inflight.controller.dispose();
  release?.({ ok: true, mediaUrl: "ws://peer/media?mediaTicket=t" });
  await inflight.settle();
  assert.deepEqual(inflight.collector.recovered, [], "dispose 后的在途结果被丢弃");
  assert.deepEqual(inflight.collector.gaveUp, []);
});
