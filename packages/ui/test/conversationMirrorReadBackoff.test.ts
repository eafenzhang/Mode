import assert from "node:assert/strict";
import test from "node:test";
import {
  CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS,
  CONVERSATION_MIRROR_READ_BACKOFF_MAX_MS,
  createConversationMirrorReadBackoff,
} from "../src/lib/conversationMirrorReadBackoff.js";

/**
 * 镜像来源读取失败的退避表（docs/specs/remote-workspace-conversation-mirror.md「第四轮修复」成因 C）。
 *
 * 这条链的关键不是「退避多久」，而是三件事：失败过的来源在冷静期内不被重读、
 * 冷静期过去后恢复重读、成功一次即清零。任何一条反过来都会把「稳定读不出来的会话」
 * 变成每轮都撞一次的错误源，或者干脆永远不再尝试。
 */

function fakeClock(start = 1_000_000) {
  let now = start;
  return {
    now: () => now,
    advance: (ms: number) => {
      now += ms;
    },
  };
}

test("未失败过的来源可直接读取", () => {
  const backoff = createConversationMirrorReadBackoff();
  assert.equal(backoff.shouldSkip("sess-1"), undefined);
});

test("失败后进入冷静期：期内跳过并带回上次失败原因", () => {
  const clock = fakeClock();
  const backoff = createConversationMirrorReadBackoff({ now: clock.now });

  backoff.recordFailure("sess-1", "ZodError: messages[67].parts[1].url");

  assert.equal(backoff.shouldSkip("sess-1"), "ZodError: messages[67].parts[1].url");
  assert.equal(backoff.shouldSkip("sess-2"), undefined, "冷静期只针对失败过的那条来源");
});

test("冷静期按失败次数指数增长并封顶", () => {
  const clock = fakeClock();
  const backoff = createConversationMirrorReadBackoff({ now: clock.now });

  backoff.recordFailure("sess-1", "boom");
  clock.advance(CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS);
  assert.equal(backoff.shouldSkip("sess-1"), undefined, "首次失败 60s 后恢复尝试");

  backoff.recordFailure("sess-1", "boom");
  clock.advance(CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS);
  assert.notEqual(backoff.shouldSkip("sess-1"), undefined, "第二次失败翻倍到 120s，60s 时仍在冷静期");

  clock.advance(CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS);
  assert.equal(backoff.shouldSkip("sess-1"), undefined);

  // 一直失败也不会退化成「永不重试」：冷静期封顶后仍会周期性再试一次。
  for (let i = 0; i < 10; i += 1) {
    backoff.recordFailure("sess-1", "boom");
  }
  clock.advance(CONVERSATION_MIRROR_READ_BACKOFF_MAX_MS - 1);
  assert.notEqual(backoff.shouldSkip("sess-1"), undefined);
  clock.advance(1);
  assert.equal(backoff.shouldSkip("sess-1"), undefined);
});

test("成功一次即清零；reset 清空全部退避（手动重试必须是真重试）", () => {
  const clock = fakeClock();
  const backoff = createConversationMirrorReadBackoff({ now: clock.now });

  backoff.recordFailure("sess-1", "boom");
  backoff.recordSuccess("sess-1");
  assert.equal(backoff.shouldSkip("sess-1"), undefined);

  backoff.recordFailure("sess-1", "boom");
  backoff.recordFailure("sess-2", "boom");
  backoff.reset();
  assert.equal(backoff.shouldSkip("sess-1"), undefined);
  assert.equal(backoff.shouldSkip("sess-2"), undefined);
});
