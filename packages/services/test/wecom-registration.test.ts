import assert from "node:assert/strict";
import test from "node:test";

// 企业微信智能机器人扫码创建：接口状态映射、scode 净化、错误分支。
// 用 stub fetch 覆盖真实网络调用；这些断言锁定的是 UI 轮询依赖的契约。

type FetchStub = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function withFetchStub<T>(
  stub: FetchStub,
  run: () => Promise<T>,
): Promise<T> {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = stub as typeof fetch;
  try {
    return await run();
  } finally {
    globalThis.fetch = originalFetch;
  }
}

test("sanitizeWeComScode 只保留字母数字（防 URL 注入）", async () => {
  const { sanitizeWeComScode } = await import("../src/bots/providers/wecomRegistration.js");
  assert.equal(sanitizeWeComScode("ab-c_d!e/f"), "abcdef");
  assert.equal(sanitizeWeComScode("---"), "");
});

test("begin 解析 scode 与 auth_url；缺失字段报错", async () => {
  const { beginWeComRegistration } = await import("../src/bots/providers/wecomRegistration.js");
  const result = await withFetchStub(
    async () =>
      jsonResponse({ errcode: 0, data: { scode: "SC123", auth_url: "https://work.weixin.qq.com/x" } }),
    () => beginWeComRegistration(),
  );
  assert.equal(result.scode, "SC123");
  assert.equal(result.authUrl, "https://work.weixin.qq.com/x");
  assert.ok(result.interval >= 1);
  assert.ok(result.expiresAt > Date.now());

  await assert.rejects(
    withFetchStub(async () => jsonResponse({ errcode: 0, data: {} }), () =>
      beginWeComRegistration(),
    ),
    /scode \/ auth_url/,
  );
});

test("begin 对业务错误码抛错", async () => {
  const { beginWeComRegistration } = await import("../src/bots/providers/wecomRegistration.js");
  await assert.rejects(
    withFetchStub(
      async () => jsonResponse({ errcode: 93000, errmsg: "invalid source" }),
      () => beginWeComRegistration(),
    ),
    /93000.*invalid source/,
  );
});

test("poll 状态映射：waiting/scaned/success/expired/cancelled/denied", async () => {
  const { pollWeComRegistration } = await import("../src/bots/providers/wecomRegistration.js");
  const cases: Array<[string, string]> = [
    ["waiting", "pending"],
    ["scaned", "scanned"],
    ["expired", "expired"],
    ["cancelled", "cancelled"],
    ["denied", "denied"],
  ];
  for (const [remote, expected] of cases) {
    const result = await withFetchStub(
      async () => jsonResponse({ errcode: 0, data: { status: remote } }),
      () => pollWeComRegistration({ scode: "SC1" }),
    );
    assert.equal(result.status, expected, `status ${remote} → ${expected}`);
  }

  const success = await withFetchStub(
    async () =>
      jsonResponse({
        errcode: 0,
        data: { status: "success", bot_info: { botid: "wb_1", secret: "sec_1" } },
      }),
    () => pollWeComRegistration({ scode: "SC1" }),
  );
  assert.deepEqual(success, { status: "success", botId: "wb_1", secret: "sec_1" });
});

test("poll：bot_info 不完整报 error；scode 全非法时不发请求", async () => {
  const { pollWeComRegistration } = await import("../src/bots/providers/wecomRegistration.js");
  const incomplete = await withFetchStub(
    async () => jsonResponse({ errcode: 0, data: { status: "success", bot_info: { botid: "wb_1" } } }),
    () => pollWeComRegistration({ scode: "SC1" }),
  );
  assert.equal(incomplete.status, "error");

  let fetchCalled = false;
  const invalid = await withFetchStub(
    async () => {
      fetchCalled = true;
      return jsonResponse({});
    },
    () => pollWeComRegistration({ scode: "!!!" }),
  );
  assert.equal(invalid.status, "error");
  assert.equal(fetchCalled, false);
});
