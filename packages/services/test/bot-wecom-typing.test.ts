import assert from "node:assert/strict";
import test from "node:test";
import { createWeComBotProvider } from "../src/bots/providers/wecomProvider.js";
import type { WeComConnectionRegistry } from "../src/bots/wecomConnection.js";
import type { BotConfig, BotOutboundMessage } from "@mode/shared";
import type { BotStreamingReplyCardState } from "../src/bots/providers/types.js";

// 企微等待期反馈（docs/specs/wecom-reply-feedback.md）：
// 企微没有原生「正在输入」信号（SDK 无 typing 命令），等待期的可见反馈只能靠
// 立刻开一条非终态 replyStream 占位（官方 SDK 示例同款）。占位文案与节奏对齐
// MyAgents 企微插件的 THINKING_MESSAGE：开流即发一次静态 <think></think>，
// 无循环打点动画；收口归属给下一条真正出站的消息——send 接管成正文，
// 流式卡片接管成首帧；stopTyping 不收口（静态占位没有动画可停）。

interface RecordedCall {
  method: "replyStream" | "replyStreamNonBlocking" | "sendMessage";
  args: unknown[];
}

interface HarnessOptions {
  withFrame?: boolean;
  /** 让第一次 replyStream 挂起，模拟 typing 打开与出站发送的并发窗口。 */
  deferFirstStream?: boolean;
}

function createHarness(options: HarnessOptions = {}) {
  const calls: RecordedCall[] = [];
  const frame =
    options.withFrame === false ? null : { headers: { req_id: "req-1" }, body: { msgid: "m1" } };
  let releaseFirstStream: (() => void) | undefined;
  const firstStreamGate = options.deferFirstStream
    ? new Promise<void>((resolve) => {
        releaseFirstStream = resolve;
      })
    : undefined;
  let firstStreamPending = options.deferFirstStream === true;
  const client = {
    replyStream(...args: unknown[]) {
      calls.push({ method: "replyStream", args });
      if (firstStreamPending) {
        firstStreamPending = false;
        return firstStreamGate!.then(() => ({}));
      }
      return Promise.resolve({});
    },
    replyStreamNonBlocking(...args: unknown[]) {
      calls.push({ method: "replyStreamNonBlocking", args });
      return Promise.resolve({});
    },
    sendMessage(...args: unknown[]) {
      calls.push({ method: "sendMessage", args });
      return Promise.resolve({});
    },
  };
  const connection = {
    client,
    getLastFrame: () => frame,
  };
  const connectionRegistry = {
    getConnection: () => connection,
    setConnection: () => undefined,
    clearConnection: () => undefined,
  } as unknown as WeComConnectionRegistry;
  const provider = createWeComBotProvider({
    loadCredential: async () => "secret",
    connection: connectionRegistry,
  });
  return { provider, calls, frame, releaseFirstStream: releaseFirstStream! };
}

const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
const chatKey = "user-1";
const target = { providerUserId: chatKey, providerMessageId: "m1" };

function outbound(text: string): BotOutboundMessage {
  return { botId: "bot-1", provider: "wecom", providerUserId: chatKey, text };
}

function streamCalls(calls: RecordedCall[]): RecordedCall[] {
  return calls.filter((call) => call.method === "replyStream");
}

test("startTyping 立即开一条非终态占位流；重复触发只开一条", async () => {
  const { provider, calls, frame } = createHarness();
  await provider.startTyping!(bot, target);
  await provider.startTyping!(bot, target);
  await provider.sendTyping!(bot, target);

  const streams = streamCalls(calls);
  assert.equal(streams.length, 1, "占位流只能开一条");
  const [usedFrame, streamId, content, finish] = streams[0]!.args as [
    unknown,
    string,
    string,
    boolean | undefined,
  ];
  assert.equal(usedFrame, frame, "必须透传入站帧的 req_id");
  assert.equal(finish, false, "占位帧必须是非终态");
  assert.equal(
    content,
    "<think></think>",
    "占位文案与 MyAgents 企微插件 THINKING_MESSAGE 逐字一致",
  );
  assert.equal(typeof streamId, "string");
});

test("无入站帧时静默降级：不开占位流也不抛错，出站照常", async () => {
  const { provider, calls } = createHarness({ withFrame: false });
  await provider.startTyping!(bot, target);
  assert.equal(streamCalls(calls).length, 0, "没有帧就没有占位流");

  await provider.send(bot, outbound("回复"));
  assert.equal(streamCalls(calls).length, 0, "降级路径不该出现 replyStream");
  assert.equal(
    calls.filter((call) => call.method === "sendMessage").length,
    1,
    "内容仍然走主动推送",
  );
});

test("出站文本接管占位流：首片 finish 收口、余片 sendMessage；未开占位时维持原推送", async () => {
  const { provider, calls, frame } = createHarness();
  await provider.startTyping!(bot, target);
  const placeholderStreamId = (streamCalls(calls)[0]!.args as [unknown, string])[1];

  const longText = `回答开头。${"内容".repeat(3000)}`;
  await provider.send(bot, outbound(longText));
  const streams = streamCalls(calls);
  assert.equal(streams.length, 2, "接管只追加一次 replyStream");
  const [usedFrame, streamId, head, finish] = streams[1]!.args as [
    unknown,
    string,
    string,
    boolean | undefined,
  ];
  assert.equal(usedFrame, frame, "收口必须沿用占位流打开时的入站帧");
  assert.equal(streamId, placeholderStreamId, "必须复用占位流的 streamId");
  assert.equal(finish, true, "首片必须 finish 收口占位流");
  assert.ok(longText.includes(head.slice(0, 8)), "首片必须是回复正文的开头");
  const pushes = calls.filter((call) => call.method === "sendMessage");
  assert.ok(pushes.length >= 1, "超长正文的余片继续分片推送");

  // 占位流已收口：下一条出站不再走 replyStream。
  await provider.send(bot, outbound("第二条"));
  assert.equal(streamCalls(calls).length, 2, "占位流已消费，不应重复收口");
});

test("流式卡片复用占位流：同一 streamId，不产生第二条消息", async () => {
  const { provider, calls } = createHarness();
  await provider.startTyping!(bot, target);
  const placeholderStreamId = (streamCalls(calls)[0]!.args as [unknown, string])[1];

  const state: BotStreamingReplyCardState = {
    providerUserId: chatKey,
    status: "running",
    blocks: [{ type: "message", text: "正在处理..." }],
  };
  const handle = await provider.createStreamingReplyCard!(bot, state);
  assert.equal(handle?.providerMessageId, placeholderStreamId, "卡片必须接管占位流");

  const createCalls = streamCalls(calls);
  assert.equal(createCalls.length, 2, "接管帧复用同一 streamId");
  assert.equal((createCalls[1]!.args as [])[1], placeholderStreamId);

  await provider.updateStreamingReplyCard!(bot, handle!, {
    ...state,
    status: "completed",
  });
  const allStreams = streamCalls(calls);
  const finalCall = allStreams[allStreams.length - 1]!.args as [unknown, string, string, boolean];
  assert.equal(finalCall[1], placeholderStreamId, "终稿仍落在同一条流上");
  assert.equal(finalCall[3], true, "终稿 finish");
});

test("占位流按 bot 隔离：同一会话里另一个企微 bot 不会误接管", async () => {
  const { provider, calls } = createHarness();
  await provider.startTyping!(bot, target);
  const otherBot = { id: "bot-2", provider: "wecom" } as unknown as BotConfig;
  await provider.send(otherBot, { ...outbound("另一个机器人"), botId: "bot-2" });

  const pushes = calls.filter((call) => call.method === "sendMessage");
  assert.equal(streamCalls(calls).length, 1, "其他 bot 的出站不得收口本 bot 的占位流");
  assert.equal(pushes.length, 1, "其他 bot 走自己的主动推送");
});

test("stopTyping 不收口占位流：静态占位没有动画可停（收口仍归出站）", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  return (async () => {
    const { provider, calls } = createHarness();
    await provider.startTyping!(bot, target);
    t.mock.timers.tick(500);
    t.mock.timers.tick(500);
    assert.equal(
      calls.filter((call) => call.method === "replyStreamNonBlocking").length,
      0,
      "静态占位不随时间发任何刷新帧",
    );

    provider.stopTyping!(bot, target);
    t.mock.timers.tick(500);
    assert.equal(
      streamCalls(calls).length,
      1,
      "stopTyping 不得 finish 占位流（没有撤回能力，收口归出站）",
    );
    assert.equal(streamCalls(calls)[0]!.args[3], false, "占位保持非终态等出站接管");

    // 出站仍能接管收口。
    await provider.send(bot, outbound("回答"));
    const streams = streamCalls(calls);
    assert.equal(streams.length, 2);
    assert.equal(streams[1]!.args[3], true, "出站 finish 收口");
  })();
});

test("startTyping 幂等：stopTyping/重复触发都不开第二条流、不发刷新帧", async () => {
  const { provider, calls } = createHarness();
  await provider.startTyping!(bot, target);
  provider.stopTyping!(bot, target);
  await provider.startTyping!(bot, target);
  await provider.sendTyping!(bot, target);

  assert.equal(streamCalls(calls).length, 1, "重复触发复用同一条静态占位流");
  assert.equal(
    calls.filter((call) => call.method === "replyStreamNonBlocking").length,
    0,
    "静态占位没有任何循环刷新帧（原「续动画」行为随打点动画一并移除）",
  );
});

test("占位是 MyAgents 同款静态 <think></think>：不跑循环帧，接管后也无残留刷新", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  return (async () => {
    const { provider, calls } = createHarness();
    await provider.startTyping!(bot, target);
    const streams = streamCalls(calls);
    assert.equal(streams.length, 1);
    assert.equal(streams[0]!.args[2], "<think></think>", "首帧即 MyAgents 同款占位文案");
    assert.equal(streams[0]!.args[3], false, "占位帧必须是非终态");

    for (let i = 0; i < 4; i += 1) {
      t.mock.timers.tick(500);
    }
    assert.equal(
      calls.filter((call) => call.method === "replyStreamNonBlocking").length,
      0,
      "静态占位不随时间刷新",
    );

    await provider.send(bot, outbound("正文"));
    t.mock.timers.tick(500);
    assert.equal(
      calls.filter((call) => call.method === "replyStreamNonBlocking").length,
      0,
      "接管后同样没有刷新帧",
    );
    assert.equal(streamCalls(calls).at(-1)?.args[3], true, "正文以 finish 收口");
  })();
});

test("typing 打开与出站并发时先等打开收敛：不会在回复之后留下占位流", async () => {
  const { provider, calls, releaseFirstStream } = createHarness({ deferFirstStream: true });
  // sendTyping 由 botsService 以 void 触发，出站可能紧随其后、早于占位帧落地。
  const typingPromise = provider.sendTyping!(bot, target);
  const sendPromise = provider.send(bot, outbound("并发回复"));
  releaseFirstStream!();
  await Promise.all([typingPromise, sendPromise]);

  const streams = streamCalls(calls);
  assert.equal(streams.length, 2, "打开 + 接管收口各一帧");
  const [openFrame, openId] = streams[0]!.args as [unknown, string];
  const [closeFrame, closeId, , finish] = streams[1]!.args as [unknown, string, string, boolean];
  assert.equal(closeId, openId, "收口必须落在刚打开的占位流上");
  assert.equal(closeFrame, openFrame, "收口沿用同一入站帧");
  assert.equal(finish, true, "占位流由这条出站收口");
  assert.equal(
    calls.filter((call) => call.method === "sendMessage").length,
    0,
    "接管成功后不应再走主动推送",
  );

  // 占位已消费：后续出站回到主动推送，不会残留可被再次收口的占位流。
  await provider.send(bot, outbound("第二条"));
  assert.equal(streamCalls(calls).length, 2, "没有残留占位流");
});
