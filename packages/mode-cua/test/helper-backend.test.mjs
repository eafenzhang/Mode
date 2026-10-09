// Task 8：createBackend 8 原语映射 + errors.mjs 17 码转译（fake addon 注入，不依赖真 UIA）。
// 裁决落点：health 由 backend 合成（pid=process.pid + protocolVersion），绝不读 addon；
// 参数校验先于 addon 调用（invalid_request 不进 napi）；napi "<code>:<message>" 错误
// 经 parseAxError 重缠为带 code 的 Error，由 errorResponseFromException 带码外发。
import test from "node:test";
import assert from "node:assert/strict";
import { dispatchRequest } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import { AX_ERROR_CODES, parseAxError } from "../helper/errors.mjs";
import { createBackend } from "../helper/server.mjs";

test("parseAxError 拆 17 码前缀，码表钉死 17 键", () => {
  // 码表 = docs/specs/computer-use-windows-runtime.md 的 17 键
  //（与 crates/mode-cua-ax/src/error.rs 的 ALLOWED_CODES 同源清单）。
  assert.deepEqual(AX_ERROR_CODES, [
    "permission_denied",
    "not_authorized",
    "launch_failed",
    "invalid_request",
    "element_unavailable",
    "not_settable",
    "not_selectable",
    "action_unavailable",
    "foreground_required",
    "controller_busy",
    "broker_unavailable",
    "version_mismatch",
    "stale_socket",
    "timeout",
    "unimplemented",
    "method_not_found",
    "internal",
  ]);
  // brief 样例：冒号后带空格。
  assert.deepEqual(parseAxError(new Error("element_unavailable: row vanished")), {
    code: "element_unavailable",
    message: "row vanished",
  });
  // 真 napi 线格式无空格（to_napi format!("{}:{}")）：同样必须拆开。
  assert.deepEqual(parseAxError(new Error("invalid_request:fullScreen=false 时必须提供 windowId")), {
    code: "invalid_request",
    message: "fullScreen=false 时必须提供 windowId",
  });
  // 无冒号 / 前缀不在 17 码表 → internal（整段原文保留，便于诊断）。
  assert.deepEqual(parseAxError(new Error("no prefix here")), {
    code: "internal",
    message: "no prefix here",
  });
  assert.equal(parseAxError(new Error("bogus_code: x")).code, "internal");
  // 只按第一个冒号切：message 内部的冒号保留。
  assert.deepEqual(parseAxError(new Error("internal: a: b")), {
    code: "internal",
    message: "a: b",
  });
});

test("perform 错误重缠带 code；health 合成且不读 addon", async () => {
  const addon = {
    perform: async () => {
      throw new Error("foreground_required: app is background");
    },
    // 裁决 1：health 永不读 addon —— 诱饵导出（读了必炸），合成路径必须绕开它。
    health: () => {
      throw new Error("internal: addon health must never be read");
    },
  };
  const res = await dispatchRequest(createBackend(addon), {
    id: "1",
    method: "perform",
    // 裁决 5：校验通过的参数才到 addon（brief 原样 params:{} 会先被 invalid_request 拒掉）。
    params: { kind: "key", windowId: 7, payload: { key: "a" } },
  });
  assert.deepEqual(res, {
    ok: false,
    error: { code: "foreground_required", message: "app is background" },
  });
  const ok = await dispatchRequest(createBackend(addon), { id: "2", method: "health", params: {} });
  assert.deepEqual(ok, {
    ok: true,
    result: { bundleId: null, pid: process.pid, protocolVersion: HELPER_PROTOCOL_VERSION },
  });
});

test("8 原语 1:1 映射：参数整形、结果透传与 launch_app Promise", async () => {
  const calls = [];
  const record = (name, result) => (...args) => {
    calls.push([name, args]);
    return result;
  };
  const addon = {
    list_apps: record("list_apps", [{ pid: 1, name: "a.exe", bundle_id: null, active: false }]),
    list_windows: record("list_windows", []),
    observe: record("observe", {
      window_title: null,
      focused_index: null,
      enumeration_complete: true,
      elements: [],
    }),
    capture: record("capture", { data: Buffer.from([137, 80]), width: 2, height: 1, clamped: true }),
    perform: record("perform", { dispatched: "dispatched" }),
    launch_app: (req) => {
      calls.push(["launch_app", [req]]);
      return Promise.resolve({ pid: 5, name: "N", bundle_id: null, active: true });
    },
    screen_probe: record("screen_probe", { locked: true }),
  };
  const backend = createBackend(addon);
  const call = (method, params) => dispatchRequest(backend, { id: method, method, params });

  assert.deepEqual(await call("list_apps", {}), {
    ok: true,
    result: [{ pid: 1, name: "a.exe", bundle_id: null, active: false }],
  });
  await call("list_windows", {});
  await call("list_windows", { pid: 4321 });
  await call("observe", { windowId: 7 });
  await call("observe", { windowId: 7, maxElements: 50 });
  await call("capture", { windowId: 3 });
  const payload = { key: "a", repeat: 2 };
  await call("perform", { kind: "key", windowId: 3, payload });
  // launch_app 是 napi AsyncTask（Promise 型，Task 6 裁决 backend 必须 await——见
  // server.mjs callAddon 的 try/await）：此处钉住 resolve 后的 AppInfo 透传与参数键省略。
  const launched = await call("launch_app", { name: "Notepad" });
  assert.deepEqual(launched, {
    ok: true,
    result: { pid: 5, name: "N", bundle_id: null, active: true },
  });
  // async rejection 同样走错误重缠（不是只有同步 throw 才带码）。
  const failing = createBackend({
    launch_app: () => Promise.reject(new Error("launch_failed: no target")),
  });
  assert.deepEqual(
    await dispatchRequest(failing, { id: "x", method: "launch_app", params: { name: "Ghost" } }),
    { ok: false, error: { code: "launch_failed", message: "no target" } },
  );
  assert.deepEqual(await call("screen_probe", {}), { ok: true, result: { locked: true } });

  assert.deepEqual(calls, [
    ["list_apps", []], // napi list_apps() 无参
    ["list_windows", []], // pid 缺席 → 省略实参（napi Option 拒绝显式 null）
    ["list_windows", [4321]],
    ["observe", [{ windowId: 7 }]], // maxElements 缺席 → 键省略（Task 3 carry，缺省 3000 在 Rust）
    ["observe", [{ windowId: 7, maxElements: 50 }]],
    ["capture", [{ windowId: 3, fullScreen: false }]], // fullScreen 缺席 → 合成 false（napi 侧必填 bool）
    ["perform", [{ kind: "key", windowId: 3, payload }]], // payload 原样透传（同一引用）
    ["launch_app", [{ name: "Notepad" }]], // bundleId 缺席 → 键省略
    ["screen_probe", []],
  ]);
  assert.equal(calls[6][1][0].payload, payload, "perform payload 必须原样透传");
});

test("参数校验在触碰 addon 之前拒绝，invalid_request 不进 napi", async () => {
  const touched = [];
  const addon = {
    observe: (req) => {
      touched.push(["observe", req]);
      return {};
    },
    perform: (req) => {
      touched.push(["perform", req]);
      return {};
    },
    capture: (req) => {
      touched.push(["capture", req]);
      return {};
    },
    launch_app: (req) => {
      touched.push(["launch_app", req]);
      return Promise.resolve({});
    },
    list_windows: (pid) => {
      touched.push(["list_windows", pid]);
      return [];
    },
  };
  const call = (method, params) =>
    dispatchRequest(createBackend(addon), { id: "1", method, params });
  const codeOf = async (method, params) => (await call(method, params)).error?.code;

  assert.deepEqual(await call("observe", {}), {
    ok: false,
    error: { code: "invalid_request", message: "windowId is required" },
  });
  assert.equal(await codeOf("observe", { windowId: "7" }), "invalid_request");
  assert.equal(await codeOf("perform", { windowId: 1, payload: {} }), "invalid_request"); // 缺 kind
  assert.equal(await codeOf("perform", { kind: "key", windowId: 1 }), "invalid_request"); // 缺 payload
  assert.equal(await codeOf("perform", { kind: 7, windowId: 1, payload: {} }), "invalid_request"); // kind 类型错
  assert.equal(await codeOf("capture", { fullScreen: "yes" }), "invalid_request");
  assert.equal(
    await codeOf("capture", { windowId: 1, region: ["a", "b", "c", "d"] }),
    "invalid_request",
  );
  assert.equal(await codeOf("launch_app", { name: 5 }), "invalid_request");
  assert.equal(await codeOf("list_windows", { pid: 1.5 }), "invalid_request");
  assert.deepEqual(touched, [], "校验失败不得触碰 addon");
});
