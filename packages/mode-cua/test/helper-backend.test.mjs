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

test("raw_input：events 原样透传、空数组合法、非法类型先拦", async () => {
  // Task 4（lan-remote-desktop）：params {events} → addon.raw_input({events}) → {dispatched}。
  const touched = [];
  const addon = {
    raw_input: (req) => {
      touched.push(req);
      return { dispatched: req.events.length };
    },
  };
  const backend = createBackend(addon);
  const call = (params) => dispatchRequest(backend, { id: "1", method: "raw_input", params });

  // 空数组是合法请求（远控空闲期的常态）：原样透传同一引用，结果原样带出。
  const empty = [];
  assert.deepEqual(await call({ events: empty }), { ok: true, result: { dispatched: 0 } });
  assert.equal(touched[0].events, empty, "events 必须原样透传（同一引用）");
  // 非空事件的形状/逐条校验归 Rust parse_raw_input（本层不复刻），backing 只做类型把关。
  const events = [{ k: "move", x: 1, y: 2 }];
  assert.deepEqual(await call({ events }), { ok: true, result: { dispatched: 1 } });
  assert.equal(touched[1].events, events);
  assert.equal(touched.length, 2, "合法请求必须到达 addon");

  // 非法类型在触碰 addon 之前拦下（napi 缺键或裸 TypeError 的防线）。
  const codeOf = async (params) => (await call(params)).error?.code;
  assert.equal(await codeOf({}), "invalid_request"); // 缺 events
  assert.equal(await codeOf({ events: "nope" }), "invalid_request");
  assert.equal(await codeOf({ events: null }), "invalid_request");
  assert.equal(await codeOf({ events: [null] }), "invalid_request");
  assert.equal(await codeOf({ events: [[{ k: "move", x: 1, y: 2 }]] }), "invalid_request");
  assert.equal(await codeOf({ events: [3] }), "invalid_request");
  assert.equal(touched.length, 2, "校验失败不得触碰 addon");
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
  // Task 8 carry：region 元素必须是 i32 整数——浮点/越界若放行，napi Vec<i32> 会 ToInt32
  // 静默截断（1.5→1、2^31→-2^31），坏区域被当成合法坐标下发；一律 invalid_request。
  assert.equal(
    await codeOf("capture", { windowId: 1, region: [1.5, 0, 10, 10] }),
    "invalid_request",
  );
  assert.equal(
    await codeOf("capture", { windowId: 1, region: [0, 0, 0x1_0000_0000, 0] }),
    "invalid_request",
  );
  assert.equal(await codeOf("launch_app", { name: 5 }), "invalid_request");
  assert.equal(await codeOf("list_windows", { pid: 1.5 }), "invalid_request");
  assert.deepEqual(touched, [], "校验失败不得触碰 addon");
});

test("rd_media_*（契约 G）：参数校验先拦、stop 未启动幂等", async () => {
  // Task 7（lan-remote-desktop-native-media）：rd_media_start/switch/stop 三个 broker 方法
  // 的入参面校验（缺参/类型错 → invalid_request，同 raw_input 范式）。本用例只验证校验与
  // stop 幂等：非法请求不得触达媒体泵（也就不会建网络连接）。
  const calls = [];
  const addon = {
    rd_media_start: (opts) => {
      calls.push(["start", opts]);
    },
    rd_media_switch: (rect) => {
      calls.push(["switch", rect]);
    },
    rd_media_stop: () => {
      calls.push(["stop"]);
    },
  };
  const backend = createBackend(addon);
  const call = (method, params) => dispatchRequest(backend, { id: "1", method, params });
  const codeOf = async (method, params) => (await call(method, params)).error?.code;

  const rect = { x: -1920, y: 0, width: 1920, height: 1080 }; // x 为负：多屏负原点合法
  const valid = {
    rect,
    maxWidth: 1920,
    fps: 60,
    bitrateBps: 8_000_000,
    endpoint: { host: "127.0.0.1", port: 1 },
    key: "11".repeat(32),
    salt: "22".repeat(8),
  };
  const without = (key) => {
    const p = { ...valid };
    delete p[key];
    return p;
  };

  // stop 未 start 即幂等 {ok:true}，且不触 addon（mediaPump 无会话直接返回）
  assert.deepEqual(await call("rd_media_stop", {}), { ok: true, result: { ok: true } });
  assert.deepEqual(calls, []);

  // start：7 个键逐一缺席 → invalid_request
  for (const key of ["rect", "maxWidth", "fps", "bitrateBps", "endpoint", "key", "salt"]) {
    assert.equal(await codeOf("rd_media_start", without(key)), "invalid_request", `缺 ${key}`);
  }
  // 类型/域错：rect 非对象、字段非 i32、数值非 u32、endpoint 缺字段、key 非字符串
  assert.equal(await codeOf("rd_media_start", { ...valid, rect: 5 }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, rect: { ...rect, y: 1.5 } }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, rect: { ...rect, width: 2 ** 31 } }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, fps: "60" }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, maxWidth: -1 }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, endpoint: {} }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, endpoint: { host: "127.0.0.1", port: 1.5 } }), "invalid_request");
  // 端口域：connect 只接受 1..65535，越界（含 0）先拦，不让 ERR_SOCKET_BAD_PORT 漏成非 17 码
  assert.equal(await codeOf("rd_media_start", { ...valid, endpoint: { host: "127.0.0.1", port: 0 } }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, endpoint: { host: "127.0.0.1", port: 65536 } }), "invalid_request");
  assert.equal(await codeOf("rd_media_start", { ...valid, key: 11 }), "invalid_request");

  // switch：缺 rect / rect 类型错 → invalid_request
  assert.equal(await codeOf("rd_media_switch", {}), "invalid_request");
  assert.equal(await codeOf("rd_media_switch", { rect: "0,0,1,1" }), "invalid_request");
  assert.equal(await codeOf("rd_media_switch", { rect: { ...rect, height: null } }), "invalid_request");

  assert.deepEqual(calls, [], "校验失败不得触碰 addon / mediaPump");
});
