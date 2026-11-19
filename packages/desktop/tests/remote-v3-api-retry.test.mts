/**
 * v3 账号 REST 传输韧性（docs/specs/remote-control-v3.md §5.4）。
 *
 * 真机复现：账号服务网络抖动时，Host 的 `ensurePeer` 交叉校验一次 `fetch failed`
 * 就把整条连接判死，客户端表现为「工作区连不上」。这里守住三件事：
 * 1. 幂等 GET（设备目录 / 对端 descriptor）网络失败自动重试一次；
 * 2. POST（refresh）绝不重试——refresh token 单次消费，重发同一 token 会被服务端
 *    判定为重放并撤销整个 token family；
 * 3. 每次请求都带超时 signal，账号服务不可达时不会无限挂起。
 *
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 */
import assert from "node:assert/strict";
import { randomBytes, randomUUID } from "node:crypto";
import test from "node:test";

import {
  RemoteV3ApiError,
  fetchRemoteV3Peer,
  listRemoteV3Devices,
  refreshRemoteV3Tokens,
} from "../src/host/remoteV3/api.js";

const SERVER = "https://remote.example.test";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/** undici 在网络层失败时抛出的形态（message + cause.code）。 */
function networkFailure(): TypeError {
  const error = new TypeError("fetch failed");
  (error as TypeError & { cause?: unknown }).cause = { code: "UND_ERR_CONNECT_TIMEOUT" };
  return error;
}

function makeFetch(handler: () => Promise<Response>): {
  fetchImpl: typeof fetch;
  calls: () => number;
} {
  let calls = 0;
  const fetchImpl = (async () => {
    calls += 1;
    return await handler();
  }) as unknown as typeof fetch;
  return { fetchImpl, calls: () => calls };
}

test("设备目录：网络失败重试一次，且每次请求都带超时 signal", async () => {
  const signals: unknown[] = [];
  let calls = 0;
  const fetchImpl = (async (_input: unknown, init?: RequestInit) => {
    calls += 1;
    signals.push(init?.signal);
    if (calls === 1) throw networkFailure();
    return jsonResponse({ items: [] });
  }) as unknown as typeof fetch;

  const devices = await listRemoteV3Devices({
    serverUrl: SERVER,
    accessToken: "token",
    fetchImpl,
  });

  assert.deepEqual(devices, []);
  assert.equal(calls, 2, "幂等 GET 网络失败必须重试一次");
  assert.equal(signals.length, 2);
  assert.ok(
    signals.every((signal) => signal instanceof AbortSignal),
    "每次请求都要挂超时 signal，否则账号服务不可达时会无限挂起",
  );
});

test("refresh：网络失败不重试（refresh token 单次消费，重发即重放）", async () => {
  const { fetchImpl, calls } = makeFetch(async () => {
    throw networkFailure();
  });

  await assert.rejects(
    refreshRemoteV3Tokens({
      serverUrl: SERVER,
      deviceId: "device-1",
      refreshToken: "refresh-1",
      fetchImpl,
    }),
    /fetch failed/u,
  );
  assert.equal(calls(), 1, "POST 绝不能自动重试，否则整个 token family 被撤销");
});

test("对端 descriptor 交叉校验：网络失败重试一次", async () => {
  const device = {
    deviceId: randomUUID(),
    name: "peer-host",
    role: "host" as const,
    platform: "desktop",
    identityKey: randomBytes(32).toString("base64url"),
  };
  let calls = 0;
  const fetchImpl = (async () => {
    calls += 1;
    if (calls === 1) throw networkFailure();
    return jsonResponse({ device });
  }) as unknown as typeof fetch;

  const peer = await fetchRemoteV3Peer({
    serverUrl: SERVER,
    accessToken: "token",
    deviceId: device.deviceId,
    fetchImpl,
  });

  assert.equal(peer.deviceId, device.deviceId);
  assert.equal(calls, 2, "Host ensurePeer 走的是这条幂等 GET");
});

test("HTTP 4xx 不重试，按 RemoteV3ApiError 抛出", async () => {
  const { fetchImpl, calls } = makeFetch(async () =>
    jsonResponse({ error: { message: "invalid token" } }, 401),
  );

  await assert.rejects(
    listRemoteV3Devices({ serverUrl: SERVER, accessToken: "bad", fetchImpl }),
    (error: unknown) =>
      error instanceof RemoteV3ApiError &&
      error.status === 401 &&
      /invalid token/u.test(error.message),
  );
  assert.equal(calls(), 1, "HTTP 错误不是网络抖动，重试只会放大失败");
});
