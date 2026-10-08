import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { tmpdir } from "node:os";
import {
  BrokerError,
  mintBrokerSocketPath, resolveBrokerSocketPath,
  callBrokerMethod, probeHelperHealth,
  parseRequestLine, okResponse, errorResponse, errorResponseFromException,
  serializeResponse, dispatchRequest, handleRequestLine,
  isBrokerMethod, isReadOnlyBrokerMethod,
} from "../broker.js";

// 平台分支：win32 铸命名管道，posix 落 tmpdir 下 .sock。repo 支持
// Windows/macOS/Linux 开发，断言不能只认 win32 形态（mintBrokerSocketPath 默认入参）。
const PIPE_PATH_RE = /^\\\\\.\\pipe\\mode-cua-[0-9a-f]{16}$/u;
const SOCK_PATH_RE = /mode-cua-[0-9a-f]{16}\.sock$/u;

function assertMintedSocketPath(p) {
  if (process.platform === "win32") {
    assert.match(p, PIPE_PATH_RE);
  } else {
    assert.match(p, SOCK_PATH_RE);
    assert.ok(p.startsWith(tmpdir()), `expected path under tmpdir(): ${p}`);
  }
}

test("mintBrokerSocketPath returns an unpredictable platform socket path", () => {
  const a = mintBrokerSocketPath(); const b = mintBrokerSocketPath();
  assertMintedSocketPath(a);
  assert.notEqual(a, b);
});

test("line codec roundtrip and rejects garbage", () => {
  const req = parseRequestLine('{"id":"1","method":"health","params":{}}');
  assert.equal(req.method, "health");
  assert.equal(parseRequestLine("not json"), undefined);
  assert.equal(parseRequestLine('{"method":1}'), undefined);
  assert.deepEqual(JSON.parse(serializeResponse(okResponse({ pid: 7 }))),
    { ok: true, result: { pid: 7 } });
});

test("method whitelist: primitives only, read-only subset", () => {
  for (const m of ["health","list_apps","list_windows","observe","capture","perform","launch_app","screen_probe"]) {
    assert.ok(isBrokerMethod(m), m);
  }
  assert.ok(!isBrokerMethod("exec"));
  assert.ok(isReadOnlyBrokerMethod("observe") && !isReadOnlyBrokerMethod("perform"));
});

test("callBrokerMethod + probeHelperHealth against a live server", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("data", (buf) => {
      const req = parseRequestLine(buf.toString("utf8"));
      const res = req.method === "health"
        ? okResponse({ bundleId: null, pid: process.pid })
        : okResponse({ pong: true });
      sock.write(serializeResponse(res) + "\n");
    });
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    assert.deepEqual(await callBrokerMethod({ socketPath, method: "ping" }), { pong: true });
    assert.deepEqual(await probeHelperHealth(socketPath, { timeoutMs: 2_000 }),
      { bundleId: null, pid: process.pid });
  } finally { await new Promise((r) => server.close(r)); }
});

test("error responses throw BrokerError with code; dispatch wraps backend throws", async () => {
  const parsed = parseRequestLine('{"id":"1","method":"nope","params":{}}');
  assert.equal(parsed.method, "nope");
  const errResp = errorResponseFromException(Object.assign(new Error("boom"), { code: "timeout" }));
  assert.equal(errResp.ok, false);
  assert.equal(errResp.error.code, "timeout");
  const backend = { health: async () => { throw Object.assign(new Error("gone"), { code: "broker_unavailable" }); } };
  const res = await dispatchRequest(backend, { id: "1", method: "health", params: {} });
  assert.deepEqual(res, { ok: false, error: { code: "broker_unavailable", message: "gone" } });
});

test("handleRequestLine answers invalid_request for malformed input", async () => {
  const res = await handleRequestLine({ health: async () => ({}) }, "not json");
  assert.deepEqual(res, { ok: false, error: { code: "invalid_request", message: "malformed request" } });
});

test("dispatchRequest rejects methods outside the whitelist", async () => {
  const res = await dispatchRequest({}, { id: "1", method: "exec", params: {} });
  assert.deepEqual(res, { ok: false, error: { code: "method_not_found", message: "unknown method: exec" } });
});

test("callBrokerMethod surfaces broker error responses as BrokerError", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("error", () => {});
    sock.on("data", () => {
      const res = errorResponse("denied by helper", { code: "permission_denied" });
      sock.write(serializeResponse(res) + "\n");
    });
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    await assert.rejects(
      callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 }),
      (e) => e instanceof BrokerError && e.code === "permission_denied" && e.message === "denied by helper",
    );
  } finally { await new Promise((r) => server.close(r)); }
});

test("callBrokerMethod rejects stale_socket when server closes without a reply", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("error", () => {});
    sock.destroy();
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    await assert.rejects(
      callBrokerMethod({ socketPath, method: "ping", timeoutMs: 2_000 }),
      (e) => e instanceof BrokerError && e.code === "stale_socket",
    );
  } finally { await new Promise((r) => server.close(r)); }
});

test("callBrokerMethod rejects with timeout when server never answers", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("error", () => {});
    // 排干请求数据：paused socket 消费不到对端 EOF，finally 的 server.close 会挂死。
    sock.resume();
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    await assert.rejects(
      callBrokerMethod({ socketPath, method: "ping", timeoutMs: 300 }),
      (e) => e instanceof BrokerError && e.code === "timeout",
    );
  } finally { await new Promise((r) => server.close(r)); }
});

test("resolveBrokerSocketPath discovers injected env socket before minting", () => {
  // 旧行为保持：注入值原样返回，不做 trim。
  assert.equal(
    resolveBrokerSocketPath({ env: { MODE_CUA_PERMISSION_BROKER_SOCKET: " X " } }),
    " X ",
  );
  // 空白值不算注入（与旧 stub 的 .trim() 判定一致）→ 回落铸造。
  assertMintedSocketPath(
    resolveBrokerSocketPath({ env: { MODE_CUA_PERMISSION_BROKER_SOCKET: "   " } }),
  );
  // 完全没有注入 → 回落铸造。
  assertMintedSocketPath(resolveBrokerSocketPath({ env: {} }));
});

test("callBrokerMethod wraps malformed broker responses as BrokerError", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("error", () => {});
    sock.on("data", () => sock.write("not json\n"));
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    await assert.rejects(
      callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 }),
      (e) =>
        e instanceof BrokerError &&
        e.code === "internal" &&
        e.message.startsWith("invalid broker response:"),
    );
  } finally { await new Promise((r) => server.close(r)); }
});

test("probeHelperHealth throws broker_unavailable when no helper answers", async () => {
  const socketPath = mintBrokerSocketPath(); // 没有监听者
  await assert.rejects(
    probeHelperHealth(socketPath, { timeoutMs: 300, pollIntervalMs: 50, perTryTimeoutMs: 100 }),
    (e) => e instanceof BrokerError && e.code === "broker_unavailable",
  );
});
