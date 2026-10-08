import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import {
  BrokerError,
  mintBrokerSocketPath, callBrokerMethod, probeHelperHealth,
  parseRequestLine, okResponse, errorResponse, errorResponseFromException,
  serializeResponse, dispatchRequest, handleRequestLine,
  isBrokerMethod, isReadOnlyBrokerMethod,
} from "../broker.js";

test("mintBrokerSocketPath returns unpredictable win32 pipe path", () => {
  const a = mintBrokerSocketPath(); const b = mintBrokerSocketPath();
  assert.match(a, /^\\\\\.\\pipe\\mode-cua-[0-9a-f]{16}$/u);
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

test("probeHelperHealth throws broker_unavailable when no helper answers", async () => {
  const socketPath = mintBrokerSocketPath(); // 没有监听者
  await assert.rejects(
    probeHelperHealth(socketPath, { timeoutMs: 300, pollIntervalMs: 50, perTryTimeoutMs: 100 }),
    (e) => e instanceof BrokerError && e.code === "broker_unavailable",
  );
});
