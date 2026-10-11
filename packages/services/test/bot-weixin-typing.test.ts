import assert from "node:assert/strict";
import test from "node:test";
import { createWeixinBotProvider } from "../src/bots/providers/weixinProvider.js";
import type { BotConfig } from "@mode/shared";

// 微信等待期反馈（docs/specs/im-typing-feedback.md）：
// 微信没有占位消息能力（iLink 无编辑接口），思考期的可见反馈只有原生
// 「正在输入…」（/getconfig 取 typing_ticket → /sendtyping status:1）。
// 钉住：1) 票缓存复用；2) 发送失败强刷新票重试一次；3) 无票必须抛错不静默；
// 4) stopTyping 发 status:2 取消，避免回复后指示滞留。

type FetchStub = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

interface RecordedCall {
  path: string;
  body: Record<string, unknown>;
}

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/**
 * 按脚本应答 iLink 接口；脚本项为 [路径后缀, 响应体]，同路径可多次出现（依次消费）。
 * 未匹配到脚本的调用直接失败，保证断言覆盖每一步。
 */
function createHarness(script: Array<[string, unknown]>) {
  const calls: RecordedCall[] = [];
  const pending = [...script];
  const fetchStub: FetchStub = async (_input, init) => {
    const url = String(_input);
    const path = ["/getconfig", "/sendtyping", "/sendmessage"].find((suffix) =>
      url.endsWith(suffix),
    );
    assert.ok(path, `unexpected iLink request: ${url}`);
    calls.push({ path, body: JSON.parse(String(init?.body ?? "{}")) as Record<string, unknown> });
    const expected = pending.shift();
    assert.ok(expected, `no scripted response for ${path}`);
    assert.equal(expected[0], path, `scripted path mismatch: expect ${expected[0]} got ${path}`);
    return jsonResponse(expected[1]);
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = fetchStub as typeof fetch;
  const provider = createWeixinBotProvider({ loadCredential: async () => "token" });
  return {
    provider,
    calls,
    restore() {
      globalThis.fetch = originalFetch;
      assert.equal(pending.length, 0, `unconsumed scripted responses: ${JSON.stringify(pending)}`);
    },
  };
}

const bot = {
  id: "bot-1",
  provider: "weixin",
  credentialRef: "ref-1",
} as unknown as BotConfig;
const target = { providerUserId: "u1@im.wechat" };

test("sendTyping 开启原生输入指示；票缓存复用，第二次不再打 getconfig", async () => {
  const harness = createHarness([
    ["/getconfig", { ret: 0, typing_ticket: "ticket-1" }],
    ["/sendtyping", { ret: 0 }],
    ["/sendtyping", { ret: 0 }],
  ]);
  try {
    await harness.provider.sendTyping!(bot, target);
    await harness.provider.sendTyping!(bot, target);

    assert.deepEqual(
      harness.calls.map((call) => call.path),
      ["/getconfig", "/sendtyping", "/sendtyping"],
      "第二次必须复用缓存票，只打一次 getconfig",
    );
    assert.equal(harness.calls[1]!.body.status, 1, "开启用 status=1");
    assert.equal(harness.calls[1]!.body.typing_ticket, "ticket-1");
    assert.equal(harness.calls[2]!.body.typing_ticket, "ticket-1", "第二次沿用同一张票");
  } finally {
    harness.restore();
  }
});

test("/sendtyping 失败按票过期处理：强刷新票后重试一次", async () => {
  const harness = createHarness([
    ["/getconfig", { ret: 0, typing_ticket: "stale-ticket" }],
    ["/sendtyping", { ret: 5, errmsg: "invalid ticket" }],
    ["/getconfig", { ret: 0, typing_ticket: "fresh-ticket" }],
    ["/sendtyping", { ret: 0 }],
  ]);
  try {
    await harness.provider.sendTyping!(bot, target);
    assert.deepEqual(
      harness.calls.map((call) => call.path),
      ["/getconfig", "/sendtyping", "/getconfig", "/sendtyping"],
      "失败必须换新票再试一次",
    );
    assert.equal(harness.calls[3]!.body.typing_ticket, "fresh-ticket");
  } finally {
    harness.restore();
  }
});

test("拿不到 typing_ticket 必须抛错（上层 warn），不允许静默 return", async () => {
  const harness = createHarness([["/getconfig", { ret: 0 }]]);
  try {
    await assert.rejects(
      harness.provider.sendTyping!(bot, target),
      /typing_ticket unavailable/,
      "无票场景要抛错让上层记录 warn",
    );
    assert.equal(
      harness.calls.filter((call) => call.path === "/sendtyping").length,
      0,
      "没有票不能发 /sendtyping",
    );
  } finally {
    harness.restore();
  }
});

test("stopTyping 发 status=2 取消指示，且复用缓存票", async () => {
  const harness = createHarness([
    ["/getconfig", { ret: 0, typing_ticket: "ticket-1" }],
    ["/sendtyping", { ret: 0 }],
    ["/sendtyping", { ret: 0 }],
  ]);
  try {
    await harness.provider.sendTyping!(bot, target);
    await harness.provider.stopTyping!(bot, target);

    const typingCalls = harness.calls.filter((call) => call.path === "/sendtyping");
    assert.equal(typingCalls.length, 2);
    assert.equal(typingCalls[0]!.body.status, 1, "开启 status=1");
    assert.equal(typingCalls[1]!.body.status, 2, "取消 status=2");
    assert.equal(
      harness.calls.filter((call) => call.path === "/getconfig").length,
      1,
      "取消复用缓存票，不重复 getconfig",
    );
  } finally {
    harness.restore();
  }
});
