/* eslint-disable max-lines -- 动作面用例按 brief 逐条平铺（目标解析中心点/位移门/坐标帧三分支/
   收据三态/复合 type/return_state 附观察/键名对照表逐字锁），断言密集不宜折叠；
   .oxlintrc 的测试豁免只覆盖 *.test.ts，本计划任务仅允许改本文件。 */
// 动作面契约测试（Plan B Task 4）：9 个变更工具 → broker perform 的载荷键名（对照 Rust
// parse_req 逐字锁：click {x,y,button,clickCount,modifiers}、click_drag {fromX..}、
// scroll {x,y,direction,amount}、key {chord,repeat,holdMs}、type_text {text}、
// set_value {elementIndex|x,y + value}、select_text {elementIndex,start,length}、
// action {elementIndex,action}、paste {text}）、目标解析（element 门→bounds 中心 floor；
// coordinate 帧绑定三分支）、收据三态（含失败携带 unknown）、复合 type OR+worst-of、
// return_state 观察与 [effect_evidence unchanged]、strategy/format 一期丢弃。
// fake broker 一律真 net server（mintBrokerSocketPath + 行分隔 JSON），往返全走真实序列化。
import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";

import { mintBrokerSocketPath } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import { createComputerUseRuntime } from "../index.js";

// ────────────────────────────────────────────── canned broker 载荷（真 addon 实测形状）

const GOOD_APPS = [
  { pid: 7, name: "Notepad", bundleId: "notepad.app", active: true },
  { pid: 8, name: "calc.exe", bundleId: null, active: false },
];
// list_windows 按 pid 过滤后的真实行（camel windowId 为真 wire 键名）。
const ALL_WINDOWS = [
  { windowId: 99, pid: 7, title: "Untitled", bounds: [0, 0, 800, 600], main: true, focused: true, onscreen: true },
  { windowId: 77, pid: 8, title: "Calculator", bounds: [0, 0, 300, 200], main: true, focused: false, onscreen: true },
];

const el = (index, kind, title, value, extra = {}) => ({
  index,
  kind,
  title,
  value,
  bounds: extra.bounds ?? [index * 10, 0, 80, 24],
  actions: extra.actions ?? [],
  enabled: extra.enabled ?? true,
  offscreen: extra.offscreen ?? false,
});

const observeResult = (elements, extra = {}) => ({
  windowTitle: extra.windowTitle ?? "Untitled",
  focusedIndex: extra.focusedIndex ?? null,
  enumerationComplete: extra.enumerationComplete ?? true,
  elements,
});

const PNG_BYTES = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.alloc(60, 1)]);
const captureResult = (bytes = PNG_BYTES, extra = {}) => ({
  data: { type: "Buffer", data: Array.from(bytes) },
  width: extra.width ?? 64,
  height: extra.height ?? 32,
  clamped: false,
});

const ctx = (sessionId, workspaceKey = "ws/action") => ({
  sessionId,
  runtimeScope: "main",
  workspaceKey,
  workspacePath: "C:/ws/action",
});

const healthOk = {
  ok: true,
  result: { bundleId: null, pid: 1234, protocolVersion: HELPER_PROTOCOL_VERSION },
};

// 真 net server：按 method 回 canned JSON，逐请求记录 {method, params} 供断言。
function startBroker(handle, socketPath = mintBrokerSocketPath()) {
  const calls = [];
  const server = net.createServer((socket) => {
    socket.on("error", () => undefined);
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      let newlineAt;
      while ((newlineAt = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, newlineAt);
        buffer = buffer.slice(newlineAt + 1);
        const request = JSON.parse(line);
        calls.push({ method: request.method, params: request.params });
        socket.write(`${JSON.stringify(handle(request.method, request.params))}\n`);
      }
    });
  });
  return new Promise((resolve) => {
    server.listen(socketPath, () =>
      resolve({
        socketPath,
        calls,
        close: () => new Promise((done) => server.close(done)),
      }),
    );
  });
}

// 默认 handle：health/list_apps 恒好；list_windows 按 pid 过滤（真 helper 语义）；
// observe/capture/perform 由 overlay 覆盖——overlay 值可以是函数（按 params/调用次序取值）。
function makeHandle(overlay = {}) {
  const resolve = (value, params) => (typeof value === "function" ? value(params) : value);
  return (method, params) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: resolve(overlay.apps, params) ?? GOOD_APPS };
    if (method === "list_windows") {
      const rows =
        resolve(overlay.windows, params) ?? ALL_WINDOWS.filter((row) => row.pid === params.pid);
      return { ok: true, result: rows };
    }
    if (method === "observe") {
      // observeResponse：需要整包响应（含错误信封）时用它；否则 observe 是 result 载荷。
      const response = resolve(overlay.observeResponse, params);
      if (response !== undefined) return response;
      return { ok: true, result: resolve(overlay.observe, params) ?? observeResult([]) };
    }
    if (method === "capture")
      return { ok: true, result: resolve(overlay.capture, params) ?? captureResult() };
    if (method === "perform") {
      const responder = overlay.perform ?? { ok: true, result: { dispatched: "dispatched" } };
      return resolve(responder, params);
    }
    return { ok: true, result: {} };
  };
}

const execute = (runtime, toolName, args, context) =>
  runtime.execute({ toolName, arguments: args ?? {}, context });

const performCalls = (broker) => broker.calls.filter((call) => call.method === "perform");
const observeCalls = (broker) => broker.calls.filter((call) => call.method === "observe");

// SDK receiptOf 的等价读（computer-use-client.mjs:170-203 候选顺序的子集，只取本任务键位）：
// result 顶层 → structuredContent → 各 text 块 JSON（含其 action_outcome 嵌套）。
// 断言一律经它读收据——证明键位落在 SDK 真实可见的读取面上，而非自说自话。
function sdkReceiptOf(result) {
  const candidates = [result];
  if (result?.structuredContent && typeof result.structuredContent === "object") {
    candidates.push(result.structuredContent);
  }
  for (const block of Array.isArray(result?.content) ? result.content : []) {
    if (block?.type !== "text" || typeof block.text !== "string") continue;
    let parsed;
    try {
      parsed = JSON.parse(block.text);
    } catch {
      continue;
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) continue;
    candidates.push(parsed);
    if (parsed.action_outcome && typeof parsed.action_outcome === "object") {
      candidates.push(parsed.action_outcome);
    }
  }
  const merged = {};
  for (const key of ["action_sent", "dispatch_status", "state_sync_status"]) {
    for (const candidate of candidates) {
      if (merged[key] === undefined && candidate[key] !== undefined) {
        merged[key] = candidate[key];
        break;
      }
    }
  }
  return merged;
}

const errorPayloadOf = (result) => JSON.parse(result.content[0].text);

// 观察种子：同一 handle 下可重复调用（overlay.observe 换值即换树）。
async function seedState(runtime, broker, context, { screenshot = false } = {}) {
  const before = broker.calls.length;
  const seeded = await execute(
    runtime,
    "get_app_state",
    screenshot
      ? { app_ref: { pid: 7 }, include_screenshot: true }
      : { app_ref: { pid: 7 } },
    context,
  );
  assert.equal(seeded.isError, false, `seed get_app_state failed: ${JSON.stringify(seeded)}`);
  assert.ok(broker.calls.length > before, "seed must reach the broker");
  return seeded;
}

// ────────────────────────────────────────────── 1. element 目标：门 → 中心（floor）

test("element 目标：位移门 → bounds 中心（floor）+ click 冻结键载荷 + delivered 收据", async () => {
  const elements = [el(4, "button", "OK", null, { bounds: [10, 20, 81, 41] })];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("a1");
    await seedState(runtime, broker, context);

    const result = await execute(
      runtime,
      "left_click",
      {
        target: { type: "element", index: 4 },
        app_ref: { pid: 7 },
        mouse_button: "right",
        click_count: 2,
        modifiers: "ctrl",
        strategy: "event", // 一期丢弃：Rust payload 无 strategy 键（parse_req 逐字核对）
        return_state: "none",
      },
      context,
    );
    assert.equal(result.isError, false, JSON.stringify(result));
    // 中心 = floor([x + w/2, y + h/2]) = floor([10+40.5, 20+20.5]) = [50, 40]；
    // 载荷键名与 Rust parse_req 逐字一致，且绝不透传 strategy。
    assert.deepEqual(performCalls(broker), [
      {
        method: "perform",
        params: {
          kind: "click",
          windowId: 99,
          payload: { x: 50, y: 40, button: "right", clickCount: 2, modifiers: "ctrl" },
        },
      },
    ]);
    assert.deepEqual(result.structuredContent, {
      action_outcome: { action_sent: true, dispatch_status: "delivered" },
      action_sent: true,
      dispatch_status: "delivered",
      state_sync_status: "unconfirmed",
    });
    assert.deepEqual(JSON.parse(result.content[0].text), {
      action_sent: true,
      dispatch_status: "delivered",
      state_sync_status: "unconfirmed",
    });
    assert.deepEqual(sdkReceiptOf(result), {
      action_sent: true,
      dispatch_status: "delivered",
      state_sync_status: "unconfirmed",
    });
    assert.equal(result.content.length, 1, "return_state 缺省 → 单文本块收据");
  } finally {
    await broker.close();
  }
});

test("element 目标：无观察 / 下标不在最新观察 → element_unavailable（门在 perform 之前拒绝）", async () => {
  const elements = [el(4, "button", "OK", null)];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("a2");

    const fresh = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(fresh.isError, true);
    assert.equal(errorPayloadOf(fresh).code, "element_unavailable");
    assert.equal(errorPayloadOf(fresh).suggested_action,
      "Re-observe with get_app_state before acting again.");
    assert.equal(performCalls(broker).length, 0, "门拒绝 → 绝不下发 perform");

    await seedState(runtime, broker, context);
    const missing = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 9 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(missing.isError, true);
    assert.equal(errorPayloadOf(missing).code, "element_unavailable");
    assert.ok(
      errorPayloadOf(missing).message.includes("the index is not in the latest observation"),
      errorPayloadOf(missing).message,
    );
    assert.equal(performCalls(broker).length, 0);
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 2. coordinate 目标：帧绑定三分支

test("coordinate 目标：frame_id 匹配放行 + 省略 frame_id 绑最近可动作帧 + 缺省字段回 Rust 默认", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("c1");
    const seeded = await seedState(runtime, broker, context, { screenshot: true });
    const frameId = seeded.structuredContent.frame_id;
    assert.equal(typeof frameId, "string");

    const withFrame = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 5, y: 7, frame_id: frameId }, app_ref: { pid: 7 } },
      context,
    );
    const withoutFrame = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 0, y: 0 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(withFrame.isError, false, JSON.stringify(withFrame));
    assert.equal(withoutFrame.isError, false, JSON.stringify(withoutFrame));
    // 缺省键按 Rust parse_req 的默认值显式回填：button "left" / clickCount 1 / modifiers ""。
    assert.deepEqual(performCalls(broker), [
      {
        method: "perform",
        params: {
          kind: "click",
          windowId: 99,
          payload: { x: 5, y: 7, button: "left", clickCount: 1, modifiers: "" },
        },
      },
      {
        method: "perform",
        params: {
          kind: "click",
          windowId: 99,
          payload: { x: 0, y: 0, button: "left", clickCount: 1, modifiers: "" },
        },
      },
    ]);
  } finally {
    await broker.close();
  }
});

test("coordinate 目标：frame_id 不符 / 无帧 / owner 不符 → invalid_request 三分支 fail-closed", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("c2");

    // 分支 1：声称的 frame_id 无从比对（会话从未持帧）→ mismatch。
    const mismatch = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1, frame_id: "no-such-frame" }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(mismatch.isError, true);
    assert.equal(errorPayloadOf(mismatch).code, "invalid_request");
    assert.ok(
      errorPayloadOf(mismatch).message.includes("frame_dispatch_identity_mismatch"),
      errorPayloadOf(mismatch).message,
    );

    // 分支 2：未给 frame_id 且会话零栅格 → 指名「no actionable frame…」。
    const noFrame = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(noFrame.isError, true);
    assert.equal(errorPayloadOf(noFrame).code, "invalid_request");
    assert.ok(
      errorPayloadOf(noFrame).message.includes("no actionable frame is available in this transport"),
      errorPayloadOf(noFrame).message,
    );

    // 分支 3：帧 owner 是窗口 99（pid 7），动作却指向 pid 8 的窗口 77 → owner mismatch。
    await seedState(runtime, broker, context, { screenshot: true });
    const owner = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1 }, app_ref: { pid: 8 } },
      context,
    );
    assert.equal(owner.isError, true);
    assert.equal(errorPayloadOf(owner).code, "invalid_request");
    assert.ok(
      errorPayloadOf(owner).message.includes("frame_dispatch_identity_mismatch"),
      errorPayloadOf(owner).message,
    );
    assert.equal(performCalls(broker).length, 0, "帧绑定失败 → 零下发");
  } finally {
    await broker.close();
  }
});

test("coordinate 目标：坐标越出帧尺寸 → invalid_request（零下发）", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("c3");
    await seedState(runtime, broker, context, { screenshot: true }); // 帧 64x32

    const outside = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 64, y: 0 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(outside.isError, true);
    assert.equal(errorPayloadOf(outside).code, "invalid_request");
    assert.ok(
      errorPayloadOf(outside).message.includes("outside the actionable frame"),
      errorPayloadOf(outside).message,
    );
    assert.equal(performCalls(broker).length, 0);
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 3. 收据三态与失败收据

test("收据三态：Rust not_dispatched / unknown → not_sent / possibly_sent（unknown 仍是成功结果）", async () => {
  const responses = [
    { ok: true, result: { dispatched: "not_dispatched" } },
    { ok: true, result: { dispatched: "unknown" } },
  ];
  const broker = await startBroker(
    makeHandle({
      observe: () => observeResult([el(4, "button", "OK", null)]),
      perform: () => responses.shift(),
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("r1");
    await seedState(runtime, broker, context);

    const notSent = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(notSent.isError, false, "not_dispatched 是成功收据，不是错误");
    assert.deepEqual(notSent.structuredContent, {
      action_outcome: { action_sent: false, dispatch_status: "not_sent" },
      action_sent: false,
      dispatch_status: "not_sent",
      state_sync_status: "unconfirmed",
    });

    const unknown = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    // possibly_sent 保持 isError:false —— SDK assertOk 见 dispatch_status==="possibly_sent"
    // 才抛 TIMEOUT + actionSent=true 强制 reobserve；runtime 提前报错会丢掉这层语义。
    assert.equal(unknown.isError, false);
    assert.deepEqual(sdkReceiptOf(unknown), {
      action_sent: true,
      dispatch_status: "possibly_sent",
      state_sync_status: "unconfirmed",
    });
  } finally {
    await broker.close();
  }
});

test("失败收据：传输类失败（timeout）→ possibly_sent 与 error 同层；语义类单步失败 → 保持 Task 2 形态", async () => {
  const responses = [
    { ok: false, error: { code: "timeout", message: "broker call timed out" } },
    { ok: false, error: { code: "foreground_required", message: "app is background" } },
  ];
  const broker = await startBroker(
    makeHandle({
      observe: () => observeResult([el(4, "button", "OK", null)]),
      perform: () => responses.shift(),
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("r2");
    await seedState(runtime, broker, context);

    // 传输类：perform 在途无应答 → 无法证明未下发 → dispatched 按 unknown 归并。
    const transport = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(transport.isError, true);
    assert.deepEqual(errorPayloadOf(transport), {
      code: "timeout",
      message: "broker call timed out",
    });
    assert.deepEqual(transport.structuredContent, {
      error: { code: "timeout" },
      action_sent: true,
      dispatch_status: "possibly_sent",
    });
    assert.deepEqual(sdkReceiptOf(transport), {
      action_sent: true,
      dispatch_status: "possibly_sent",
    });

    // 语义类（Rust 校验/前台门在注入前）：无任何已归并收据 → 纯 Task 2 错误形态。
    const semantic = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(semantic.isError, true);
    assert.deepEqual(semantic.structuredContent, {
      error: {
        code: "foreground_required",
        suggested_action:
          "Target the element index instead, or bring the app to the foreground first.",
      },
    });
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 4. 复合 type：click → type_text

test("复合 type：target 在场 → click→type_text 两次 perform；收据 action_sent=OR、status=worst-of", async () => {
  let clickDispatched = "not_dispatched";
  let typeDispatched = "dispatched";
  const broker = await startBroker(
    makeHandle({
      observe: () => observeResult([el(4, "textbox", "name", "hello world")]),
      perform: (params) => ({
        ok: true,
        result: {
          dispatched: params.kind === "click" ? clickDispatched : typeDispatched,
        },
      }),
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t1");
    await seedState(runtime, broker, context);

    const first = await execute(
      runtime,
      "type",
      { text: "hi", target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(first.isError, false, JSON.stringify(first));
    const calls = performCalls(broker);
    assert.equal(calls.length, 2);
    assert.equal(calls[0].params.kind, "click");
    assert.deepEqual(calls[0].params.payload, {
      x: 80, // 默认 bounds [40,0,80,24] 中心 = floor([40+40, 0+12])
      y: 12,
      button: "left",
      clickCount: 1,
      modifiers: "",
    });
    assert.equal(calls[1].params.kind, "type_text");
    assert.deepEqual(calls[1].params.payload, { text: "hi" });
    // OR: not_dispatched ⊕ dispatched → action_sent true；worst-of: delivered 压过 not_sent。
    assert.deepEqual(sdkReceiptOf(first), {
      action_sent: true,
      dispatch_status: "delivered",
      state_sync_status: "unconfirmed",
    });

    // worst-of：click dispatched ⊕ type unknown → possibly_sent 胜出。
    clickDispatched = "dispatched";
    typeDispatched = "unknown";
    const second = await execute(
      runtime,
      "type",
      { text: "yo", target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(second.isError, false);
    assert.equal(second.structuredContent.dispatch_status, "possibly_sent");
    assert.equal(second.structuredContent.action_sent, true);

    // 双 not_dispatched → action_sent false / not_sent。
    clickDispatched = "not_dispatched";
    typeDispatched = "not_dispatched";
    const third = await execute(
      runtime,
      "type",
      { text: "!", target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.deepEqual(sdkReceiptOf(third), {
      action_sent: false,
      dispatch_status: "not_sent",
      state_sync_status: "unconfirmed",
    });

    // target 缺席 → 只有 type_text 一次 perform（不做聚焦 click）。
    const before = performCalls(broker).length;
    const solo = await execute(runtime, "type", { text: "solo", app_ref: { pid: 7 } }, context);
    assert.equal(solo.isError, false);
    const soloCalls = performCalls(broker).slice(before);
    assert.equal(soloCalls.length, 1);
    assert.equal(soloCalls[0].params.kind, "type_text");
    assert.deepEqual(soloCalls[0].params.payload, { text: "solo" });
  } finally {
    await broker.close();
  }
});

test("复合 type：click 已 dispatched 而 type_text 语义失败 → 错误携带已归并收据", async () => {
  const broker = await startBroker(
    makeHandle({
      observe: () => observeResult([el(4, "textbox", "name", null)]),
      perform: (params) =>
        params.kind === "click"
          ? { ok: true, result: { dispatched: "dispatched" } }
          : { ok: false, error: { code: "foreground_required", message: "app is background" } },
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t2");
    await seedState(runtime, broker, context);

    const result = await execute(
      runtime,
      "type",
      { text: "hi", target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(result.isError, true);
    assert.deepEqual(result.structuredContent, {
      error: {
        code: "foreground_required",
        suggested_action:
          "Target the element index instead, or bring the app to the foreground first.",
      },
      // click 的 dispatched 已归并：失败也要让 SDK 知道动作面动过（retry=reobserve）。
      action_sent: true,
      dispatch_status: "delivered",
    });
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 5. return_state 与 [effect_evidence unchanged]

test("return_state 缺省 none：动作后不追加观察；result 保持单收据文本块", async () => {
  const broker = await startBroker(
    makeHandle({ observe: () => observeResult([el(4, "button", "OK", null)]) }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s1");
    await seedState(runtime, broker, context);
    const before = observeCalls(broker).length;

    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(result.isError, false);
    assert.equal(observeCalls(broker).length, before, "return_state 缺省 → 零追加观察");
    assert.equal(result.content.length, 1);
  } finally {
    await broker.close();
  }
});

test("return_state=compact：附带 diff 观察 + 树未变 → 尾注 [effect_evidence unchanged]", async () => {
  const elements = [el(4, "button", "OK", null)];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s2");
    await seedState(runtime, broker, context);
    const observesBefore = observeCalls(broker).length;

    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 }, return_state: "compact" },
      context,
    );
    assert.equal(result.isError, false, JSON.stringify(result));
    assert.equal(observeCalls(broker).length, observesBefore + 1, "compact → 一次动作后观察");
    assert.equal(observeCalls(broker).at(-1).params.windowId, 99);

    // content = [收据文本块, 树文本块]；structuredContent 并入观察全集 + 收据键。
    assert.equal(result.content.length, 2);
    assert.deepEqual(JSON.parse(result.content[0].text), {
      action_sent: true,
      dispatch_status: "delivered",
      state_sync_status: "unconfirmed",
    });
    const tree = result.content[1].text;
    assert.ok(tree.startsWith("app: Notepad pid=7 "), tree.slice(0, 80));
    // diff 树只列变更行：全空 → 只有 diff 头 + 尾注（T3 渲染语义）。
    assert.ok(tree.includes("changes: +0 -0 ~0"), tree);
    assert.ok(!tree.includes("[4] button OK"), tree);
    assert.ok(tree.endsWith("\n[effect_evidence unchanged]"), tree);

    const structured = result.structuredContent;
    assert.equal(structured.snapshot_mode, "diff");
    assert.equal(typeof structured.state_id, "string");
    assert.equal(structured.elements.length, 1);
    assert.deepEqual(structured.changes, {
      added_count: 0,
      removed_count: 0,
      changed_count: 0,
      added: [],
      removed: [],
      changed: [],
    });
    assert.equal(structured.action_sent, true);
    assert.equal(structured.dispatch_status, "delivered");
    assert.equal(structured.state_sync_status, "unconfirmed");
    assert.deepEqual(structured.action_outcome, {
      action_sent: true,
      dispatch_status: "delivered",
    });
  } finally {
    await broker.close();
  }
});

test("return_state=compact：动作后树有变化 → 不注 unchanged 行，changes 计数如实", async () => {
  let current = [el(4, "button", "OK", null)];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(current) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s3");
    await seedState(runtime, broker, context);
    current = [el(4, "button", "OK", null), el(5, "button", "Cancel", null)];

    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 }, return_state: "compact" },
      context,
    );
    assert.equal(result.isError, false, JSON.stringify(result));
    const tree = result.content[1].text;
    assert.ok(!tree.includes("[effect_evidence unchanged]"), tree);
    assert.ok(tree.includes("[5] button Cancel"), tree);
    assert.equal(result.structuredContent.changes.added_count, 1);
    assert.equal(result.structuredContent.changes.changed_count, 0);
  } finally {
    await broker.close();
  }
});

test("return_state=full：全量观察（snapshot_mode full）+ 树未变仍注 unchanged 行", async () => {
  const elements = [el(4, "button", "OK", null), el(5, "button", "Cancel", null)];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s4");
    await seedState(runtime, broker, context);

    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 }, return_state: "full" },
      context,
    );
    assert.equal(result.isError, false, JSON.stringify(result));
    assert.equal(result.structuredContent.snapshot_mode, "full");
    assert.ok(result.content[1].text.includes("[4] button OK = null"));
    assert.ok(result.content[1].text.includes("[5] button Cancel = null"));
    // full 模式没有内置 changes —— 注行依据是「动作前基线 vs 动作后观察」的独立 diff。
    assert.ok(result.content[1].text.endsWith("\n[effect_evidence unchanged]"));
    assert.equal("changes" in result.structuredContent, false);
  } finally {
    await broker.close();
  }
});

test("return_state：动作后观察失败 → 错误携带已归并收据（delivered，防盲重试）", async () => {
  let observes = 0;
  const broker = await startBroker(
    makeHandle({
      // 第 1 次（种子）正常，第 2 次（动作后观察）整包错误。
      observeResponse: () => {
        observes += 1;
        return observes > 1
          ? { ok: false, error: { code: "internal", message: "observe exploded" } }
          : { ok: true, result: observeResult([el(4, "button", "OK", null)]) };
      },
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s5");
    await seedState(runtime, broker, context);

    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 }, return_state: "compact" },
      context,
    );
    assert.equal(result.isError, true);
    assert.deepEqual(errorPayloadOf(result), { code: "internal", message: "observe exploded" });
    assert.deepEqual(result.structuredContent, {
      error: { code: "internal" },
      action_sent: true,
      dispatch_status: "delivered",
    });
    assert.deepEqual(sdkReceiptOf(result), {
      action_sent: true,
      dispatch_status: "delivered",
    });
  } finally {
    await broker.close();
  }
});

test("return_state 取值非法 → invalid_request 且零 perform", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s6");
    const result = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1 }, app_ref: { pid: 7 }, return_state: "sometimes" },
      context,
    );
    assert.equal(result.isError, true);
    assert.equal(errorPayloadOf(result).code, "invalid_request");
    assert.equal(performCalls(broker).length, 0);
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 6. 各工具键名对照表（SDK ↔ runtime ↔ Rust）

test("key：text→chord、hold_seconds→holdMs、repeat 直通；strategy 不进载荷", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k1");
    await seedState(runtime, broker, context);

    await execute(runtime, "key", {
      text: "ctrl+a",
      repeat: 3,
      hold_seconds: 0.5,
      strategy: "a11y",
      app_ref: { pid: 7 },
    }, context);
    await execute(runtime, "key", { text: "ctrl+c", app_ref: { pid: 7 } }, context);

    assert.deepEqual(performCalls(broker), [
      {
        method: "perform",
        params: {
          kind: "key",
          windowId: 99,
          payload: { chord: "ctrl+a", repeat: 3, holdMs: 500 },
        },
      },
      {
        method: "perform",
        params: {
          kind: "key",
          windowId: 99,
          payload: { chord: "ctrl+c", repeat: 1, holdMs: 0 },
        },
      },
    ]);
  } finally {
    await broker.close();
  }
});

test("scroll：scroll_direction→direction、scroll_amount→amount", async () => {
  const elements = [el(4, "list", "items", null)];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k2");
    await seedState(runtime, broker, context);

    const result = await execute(runtime, "scroll", {
      target: { type: "element", index: 4 },
      scroll_direction: "down",
      scroll_amount: 3,
      strategy: "event",
      app_ref: { pid: 7 },
    }, context);
    assert.equal(result.isError, false, JSON.stringify(result));
    assert.deepEqual(performCalls(broker).at(-1).params, {
      kind: "scroll",
      windowId: 99,
      payload: { x: 80, y: 12, direction: "down", amount: 3 },
    });

    const bad = await execute(runtime, "scroll", {
      target: { type: "element", index: 4 },
      scroll_direction: "downwards",
      scroll_amount: 1,
      app_ref: { pid: 7 },
    }, context);
    assert.equal(bad.isError, true);
    assert.equal(errorPayloadOf(bad).code, "invalid_request");
  } finally {
    await broker.close();
  }
});

test("left_click_drag：from_target/to → fromX..toY + modifiers", async () => {
  const elements = [
    el(4, "listitem", "row A", null, { bounds: [0, 0, 10, 10] }),
    el(5, "listitem", "row B", null, { bounds: [100, 50, 10, 10] }),
  ];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k3");
    await seedState(runtime, broker, context);

    const result = await execute(runtime, "left_click_drag", {
      from_target: { type: "element", index: 4 },
      to: { type: "element", index: 5 },
      modifiers: "ctrl",
      app_ref: { pid: 7 },
    }, context);
    assert.equal(result.isError, false, JSON.stringify(result));
    assert.deepEqual(performCalls(broker).at(-1).params, {
      kind: "click_drag",
      windowId: 99,
      payload: { fromX: 5, fromY: 5, toX: 105, toY: 55, modifiers: "ctrl" },
    });
  } finally {
    await broker.close();
  }
});

test("set_value：element → elementIndex；coordinate → 聚焦点 x/y（value 直通）", async () => {
  const elements = [el(4, "textbox", "name", null)];
  const broker = await startBroker(
    makeHandle({ observe: () => observeResult(elements) }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k4");
    await seedState(runtime, broker, context, { screenshot: true });

    await execute(runtime, "set_value", {
      target: { type: "element", index: 4 },
      value: "abc",
      strategy: "event",
      app_ref: { pid: 7 },
    }, context);
    await execute(runtime, "set_value", {
      target: { type: "coordinate", x: 12, y: 6 },
      value: "xyz",
      app_ref: { pid: 7 },
    }, context);

    const calls = performCalls(broker);
    assert.deepEqual(calls[0].params, {
      kind: "set_value",
      windowId: 99,
      payload: { elementIndex: 4, value: "abc" },
    });
    assert.deepEqual(calls[1].params, {
      kind: "set_value",
      windowId: 99,
      payload: { x: 12, y: 6, value: "xyz" },
    });
  } finally {
    await broker.close();
  }
});

test("select_text：text_range 解构为 start/length；缺省取观察值全长；负 length / 坐标目标拒绝", async () => {
  const elements = [
    el(4, "textbox", "name", "hello world"),
    el(5, "textbox", "empty", null),
  ];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k5");
    await seedState(runtime, broker, context);

    await execute(runtime, "select_text", {
      target: { type: "element", index: 4 },
      text_range: [2, 5],
      app_ref: { pid: 7 },
    }, context);
    // 无 text_range → 整选观察到的 value（Rust 拒负 length：perform.rs select_text 首行校验）。
    await execute(runtime, "select_text", {
      target: { type: "element", index: 4 },
      app_ref: { pid: 7 },
    }, context);
    // value 为 null → 全长按 0（Rust 允许 length 0；元素无值时无从选起）。
    await execute(runtime, "select_text", {
      target: { type: "element", index: 5 },
      app_ref: { pid: 7 },
    }, context);

    const calls = performCalls(broker);
    assert.deepEqual(calls.map((call) => call.params.payload), [
      { elementIndex: 4, start: 2, length: 5 },
      { elementIndex: 4, start: 0, length: 11 },
      { elementIndex: 5, start: 0, length: 0 },
    ]);
    assert.ok(calls.every((call) => call.params.kind === "select_text"));

    const negative = await execute(runtime, "select_text", {
      target: { type: "element", index: 4 },
      text_range: [2, -1],
      app_ref: { pid: 7 },
    }, context);
    assert.equal(negative.isError, true);
    assert.equal(errorPayloadOf(negative).code, "invalid_request");

    const coordinate = await execute(runtime, "select_text", {
      target: { type: "coordinate", x: 1, y: 1 },
      app_ref: { pid: 7 },
    }, context);
    assert.equal(coordinate.isError, true);
    assert.equal(errorPayloadOf(coordinate).code, "invalid_request");
    assert.ok(errorPayloadOf(coordinate).message.includes("element target"));
    assert.equal(performCalls(broker).length, 3, "拒绝类分支零下发");
  } finally {
    await broker.close();
  }
});

test("perform_action：action → {elementIndex, action}；坐标目标拒绝", async () => {
  const elements = [el(4, "button", "OK", null, { actions: ["press"] })];
  const broker = await startBroker(makeHandle({ observe: () => observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k6");
    await seedState(runtime, broker, context);

    const ok = await execute(runtime, "perform_action", {
      target: { type: "element", index: 4 },
      action: "press",
      app_ref: { pid: 7 },
    }, context);
    assert.equal(ok.isError, false, JSON.stringify(ok));
    assert.deepEqual(performCalls(broker).at(-1).params, {
      kind: "action",
      windowId: 99,
      payload: { elementIndex: 4, action: "press" },
    });

    const coordinate = await execute(runtime, "perform_action", {
      target: { type: "coordinate", x: 1, y: 1 },
      action: "press",
      app_ref: { pid: 7 },
    }, context);
    assert.equal(coordinate.isError, true);
    assert.equal(errorPayloadOf(coordinate).code, "invalid_request");
    assert.equal(performCalls(broker).length, 1, "坐标目标拒绝 → 零下发");
  } finally {
    await broker.close();
  }
});

test("paste：format 一期丢弃 —— 载荷只有 {text}", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("k7");
    await seedState(runtime, broker, context);

    const result = await execute(runtime, "paste", {
      text: "hello",
      format: "html",
      app_ref: { pid: 7 },
    }, context);
    assert.equal(result.isError, false, JSON.stringify(result));
    assert.deepEqual(performCalls(broker).at(-1).params, {
      kind: "paste",
      windowId: 99,
      payload: { text: "hello" },
    });
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 7. app/窗口解析与参数校验

test("app 解析：app_ref pid → 该 pid 主窗口；无 app_ref 走会话最近绑定（零解析调用）；皆无 → 点名 app_ref", async () => {
  const broker = await startBroker(
    makeHandle({ observe: () => observeResult([el(4, "button", "OK", null)]) }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("w1");

    // 分支 1：app_ref {pid:8} → list_windows(8) 的主窗口 77（paste 无目标，最直接验窗口解析）。
    const paste8 = await execute(runtime, "paste", { text: "x", app_ref: { pid: 8 } }, context);
    assert.equal(paste8.isError, false, JSON.stringify(paste8));
    assert.equal(performCalls(broker).at(-1).params.windowId, 77);

    // 分支 2：会话最近绑定——先观察 pid 7，再无 app_ref 动作 → 窗口 99，且零解析调用。
    await seedState(runtime, broker, context);
    const before = broker.calls.length;
    const bound = await execute(runtime, "paste", { text: "y" }, context);
    assert.equal(bound.isError, false, JSON.stringify(bound));
    const after = broker.calls.slice(before);
    assert.equal(after.length, 1, `bound 兜底不得打解析调用: ${JSON.stringify(after)}`);
    assert.equal(after[0].method, "perform");
    assert.equal(after[0].params.windowId, 99);

    // 分支 3：全新会话无 app_ref → invalid_request 点名 app_ref，零 perform。
    // 终审 I2：w1 已在分支 1 抢占 controller lease——先由 owner 释放（closeSession），
    // 否则新会话先命中 controller_busy（租约闸门先于 handler），验不到 app_ref 缺失面。
    await runtime.closeSession(context);
    const performsBefore = performCalls(broker).length;
    const missing = await execute(runtime, "paste", { text: "z" }, ctx("w1-fresh"));
    assert.equal(missing.isError, true);
    assert.equal(errorPayloadOf(missing).code, "invalid_request");
    assert.ok(errorPayloadOf(missing).message.includes("app_ref"), errorPayloadOf(missing).message);
    assert.equal(performCalls(broker).length, performsBefore, "缺 app_ref → 零下发");
  } finally {
    await broker.close();
  }
});

test("参数校验：缺 target / 缺 text → invalid_request 且零 perform；stop 后变更类仍 controller_busy", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("v1");

    const noTarget = await execute(runtime, "left_click", { app_ref: { pid: 7 } }, context);
    assert.equal(noTarget.isError, true);
    assert.equal(errorPayloadOf(noTarget).code, "invalid_request");
    assert.ok(errorPayloadOf(noTarget).message.includes("target"));

    const noText = await execute(runtime, "type", { app_ref: { pid: 7 } }, context);
    assert.equal(noText.isError, true);
    assert.equal(errorPayloadOf(noText).code, "invalid_request");
    assert.ok(errorPayloadOf(noText).message.includes("text"));
    assert.equal(performCalls(broker).length, 0);

    // stop 闸门先于动作处理器（Task 2 M5 carry 的三态键位在 stop 收据里另测）。
    const stop = await execute(runtime, "stop_computer_control", {}, context);
    assert.equal(stop.isError, false);
    assert.deepEqual(stop.structuredContent, {
      stopped: true,
      action_sent: true,
      dispatch_status: "delivered",
    });
    const busy = await execute(runtime, "left_click", { target: { type: "element", index: 4 } }, context);
    assert.equal(busy.isError, true);
    assert.equal(errorPayloadOf(busy).code, "controller_busy");
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── T4-M1 / T4-M2（Task 7 carry）

// T4-M1（Task 4 评审 carry）：过期 frame_id——会话曾持帧 A，再次截图观察后 lastFrame 被 B
// 替换，动作仍声称 A → invalid_request + frame_dispatch_identity_mismatch（零下发）；
// 新鲜帧 B 同场景放行（证明拒绝的是「过期」而非「有帧」）。
test("T4-M1 coordinate 目标：过期 frame_id（已被新帧替换）→ invalid_request mismatch 且零下发", async () => {
  const broker = await startBroker(makeHandle({ observe: () => observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t4m1");
    const first = await seedState(runtime, broker, context, { screenshot: true });
    const staleFrameId = first.structuredContent.frame_id;
    const second = await seedState(runtime, broker, context, { screenshot: true });
    const freshFrameId = second.structuredContent.frame_id;
    assert.equal(typeof staleFrameId, "string");
    assert.notEqual(staleFrameId, freshFrameId, "第二次截图必须替换会话 lastFrame");

    const stale = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1, frame_id: staleFrameId }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(stale.isError, true, JSON.stringify(stale));
    assert.equal(errorPayloadOf(stale).code, "invalid_request");
    assert.ok(
      errorPayloadOf(stale).message.includes("frame_dispatch_identity_mismatch"),
      errorPayloadOf(stale).message,
    );
    // message 点名过期帧 id（诊断现场：expired/replaced 分支，非 owner 分支）。
    assert.ok(errorPayloadOf(stale).message.includes(staleFrameId), errorPayloadOf(stale).message);
    assert.equal(performCalls(broker).length, 0, "过期帧拒绝 → 零下发");

    const fresh = await execute(
      runtime,
      "left_click",
      { target: { type: "coordinate", x: 1, y: 1, frame_id: freshFrameId }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(fresh.isError, false, JSON.stringify(fresh));
    assert.equal(performCalls(broker).length, 1, "新鲜帧放行 → 恰一次下发");
  } finally {
    await broker.close();
  }
});

// T4-M2（Task 4 评审 §9.2 carry）：set_value 是唯一可能在「已注入」之后抛语义错的工具——
// event 兜底（crates/mode-cua-ax/src/perform.rs event_set_value：click 聚焦已下发后才跑
// select_all_and_type）的 uipi/前台两道闸门（input.rs → action_unavailable /
// foreground_required）与 click 自身的前置同名闸门码级无法二选一（错误响应不带 dispatched）。
// 按 spec「无法证明未下发 → possibly_sent」，set_value 对这两个码归并 unknown；其余八工具
// 逐 arm 复核均为注入前失败（task-7 报告 T4-M2 节），维持不带收据的 Task 2 形态。
test("T4-M2 set_value：可能 post-dispatch 的语义失败 → possibly_sent；同码 left_click 仍纯错误形态", async () => {
  const responses = [
    { ok: false, error: { code: "foreground_required", message: "select_all gate after click" } },
    { ok: false, error: { code: "action_unavailable", message: "uipi gate after click" } },
    { ok: false, error: { code: "not_settable", message: "readonly before SetValue" } },
  ];
  const broker = await startBroker(
    makeHandle({
      observe: () => observeResult([el(4, "edit", "field", null)]),
      perform: () =>
        responses.shift() ??
        { ok: false, error: { code: "foreground_required", message: "pre-injection gate" } },
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t4m2");
    await seedState(runtime, broker, context);

    // foreground_required：event 兜底可在 click 注入后抛 → 收据与 error 同层。
    const fg = await execute(
      runtime,
      "set_value",
      { target: { type: "element", index: 4 }, value: "x", app_ref: { pid: 7 } },
      context,
    );
    assert.equal(fg.isError, true, JSON.stringify(fg));
    assert.equal(errorPayloadOf(fg).code, "foreground_required");
    assert.deepEqual(fg.structuredContent, {
      error: {
        code: "foreground_required",
        suggested_action:
          "Target the element index instead, or bring the app to the foreground first.",
      },
      action_sent: true,
      dispatch_status: "possibly_sent",
    });
    // SDK receiptOf 读取面（等价读）：不再断言「绝未下发」。
    assert.deepEqual(sdkReceiptOf(fg), { action_sent: true, dispatch_status: "possibly_sent" });

    // action_unavailable（同臂 uipi 闸门）同样归并 unknown。
    const uipi = await execute(
      runtime,
      "set_value",
      { target: { type: "element", index: 4 }, value: "y", app_ref: { pid: 7 } },
      context,
    );
    assert.equal(uipi.isError, true, JSON.stringify(uipi));
    assert.equal(errorPayloadOf(uipi).code, "action_unavailable");
    assert.deepEqual(sdkReceiptOf(uipi), { action_sent: true, dispatch_status: "possibly_sent" });

    // 对照 1：not_settable（ValuePattern 只读 / 空包围矩，注入前可证）→ 不带收据。
    const notSettable = await execute(
      runtime,
      "set_value",
      { target: { type: "element", index: 4 }, value: "z", app_ref: { pid: 7 } },
      context,
    );
    assert.equal(notSettable.isError, true, JSON.stringify(notSettable));
    assert.equal(errorPayloadOf(notSettable).code, "not_settable");
    assert.equal(notSettable.structuredContent.action_sent, undefined);
    assert.deepEqual(sdkReceiptOf(notSettable), {});

    // 对照 2：left_click 同码（该工具全臂均为注入前失败）→ 保持 Task 2 纯错误形态。
    const click = await execute(
      runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      context,
    );
    assert.equal(click.isError, true, JSON.stringify(click));
    assert.deepEqual(click.structuredContent, {
      error: {
        code: "foreground_required",
        suggested_action:
          "Target the element index instead, or bring the app to the foreground first.",
      },
    });
    assert.deepEqual(sdkReceiptOf(click), {});
  } finally {
    await broker.close();
  }
});
