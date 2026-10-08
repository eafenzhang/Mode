import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import type { ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";

// 局域网令牌 cookie 改名（zcode_lite_token → mode_lite_token）后的读写口径：
// 服务端读新名 + 读旧名兜底（老客户端留下的 cookie 仍能用），写只写新名。
// 用最小 services 桩启动真实 HTTP 服务，只打 /api/server-info（不触碰业务服务）。
async function startServer(authToken: string) {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    authToken,
    host: "127.0.0.1",
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  return { server, baseUrl: `http://127.0.0.1:${port}` };
}

test("LAN cookie：token 保护路径只认新 cookie 名或旧名兜底，写 cookie 一律用新名", async () => {
  const authToken = "lan-secret-token";
  const { server, baseUrl } = await startServer(authToken);
  try {
    const unauthenticated = await fetch(`${baseUrl}/api/server-info`);
    assert.equal(unauthenticated.status, 401, "无令牌必须拒绝");

    // 改名前的旧 cookie：仍需放行（老客户端/手机浏览器升级后不必重新配对）。
    const legacyCookie = await fetch(`${baseUrl}/api/server-info`, {
      headers: { cookie: `zcode_lite_token=${encodeURIComponent(authToken)}` },
    });
    assert.equal(legacyCookie.status, 200, "旧 cookie 名必须兜底放行");

    // 当前 cookie 名。
    const currentCookie = await fetch(`${baseUrl}/api/server-info`, {
      headers: { cookie: `mode_lite_token=${encodeURIComponent(authToken)}` },
    });
    assert.equal(currentCookie.status, 200, "新 cookie 名必须放行");

    // 从 query 令牌换 cookie 时只下发新名。
    const fromQuery = await fetch(`${baseUrl}/api/server-info?token=${encodeURIComponent(authToken)}`);
    assert.equal(fromQuery.status, 200);
    const setCookie = fromQuery.headers.get("set-cookie") ?? "";
    assert.ok(setCookie.includes("mode_lite_token="), `Set-Cookie 必须写新 cookie 名：${setCookie}`);
    assert.ok(!setCookie.includes("zcode_lite_token="), "不得再下发改名前的 cookie 名");

    // 错误令牌两种名字都不放行。
    const badCookie = await fetch(`${baseUrl}/api/server-info`, {
      headers: { cookie: "zcode_lite_token=wrong" },
    });
    assert.equal(badCookie.status, 401);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
