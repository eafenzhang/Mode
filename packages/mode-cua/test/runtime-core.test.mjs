/* eslint-disable max-lines -- 契约用例逐条平铺（错误装配逐字锁、预检/信封/闸门/会话键），断言密集
   不宜折叠；.oxlintrc 的测试豁免只覆盖 *.test.ts，本计划任务仅允许改本文件。 */
// runtime 骨架契约测试（Plan B Task 2）：execute 三形态结果、17 码错误装配（逐字锁）、
// health 版本比对粘滞、冷启动 CUA_NOT_READY 信封（与 SDK notReadyEnvelopeOf 逐字对齐）、
// stop_computer_control 闸门与会话键隔离、dispose/closeSession 生命周期、五个直通工具。
// fake broker 一律真 net server（mintBrokerSocketPath + 行分隔 JSON），往返全走真实序列化。
import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";

import { mintBrokerSocketPath } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import { AX_ERROR_CODES } from "../helper/errors.mjs";
import { createComputerUseRuntime } from "../index.js";

const GOOD_APPS = [
  { pid: 7, name: "Notepad", bundle_id: "notepad.app", active: true },
  { pid: 8, name: "calc.exe", bundle_id: null, active: false },
];
const GOOD_WINDOWS = [{ window_id: 99, pid: 7, title: "Untitled", main: true }];

// d.ts 形状（camelCase，host parseContext 实际下发的键）。
const ctx = (sessionId, workspaceKey = "ws/main") => ({
  sessionId,
  runtimeScope: "main",
  workspaceKey,
  workspacePath: "C:/ws/main",
});

const healthOk = {
  ok: true,
  result: { bundleId: null, pid: 1234, protocolVersion: HELPER_PROTOCOL_VERSION },
};
const defaultHandle = (method) => {
  if (method === "health") return healthOk;
  if (method === "list_apps") return { ok: true, result: GOOD_APPS };
  if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
  return { ok: true, result: {} };
};

// 真 net server：按 method 回 canned JSON，逐请求记录 {method, params} 供断言。
// socketPath 可显式传入（冷启动用例要在同一路径上「先无监听、后起 broker」重放）。
function startBroker(handle = defaultHandle, socketPath = mintBrokerSocketPath()) {
  const calls = [];
  const server = net.createServer((socket) => {
    socket.on("error", () => undefined);
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk;
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

const execute = (runtime, toolName, args, context) =>
  runtime.execute({ toolName, arguments: args ?? {}, context });

// ────────────────────────────────────────────── 1. 未知工具 / 生命周期

test("未知工具 → method_not_found 错误结果（文本 JSON + structuredContent.error 双落点）", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "exec", {}, ctx("s1"));
    assert.equal(result.isError, true);
    assert.equal(result.content.length, 1);
    assert.equal(
      result.content[0].text,
      JSON.stringify({ code: "method_not_found", message: "unknown tool: exec" }),
    );
    // 无映射的码不得带 suggested_action（双落点都省略）。
    assert.deepEqual(result.structuredContent, { error: { code: "method_not_found" } });
  } finally {
    await broker.close();
  }
});

test("dispose 后 execute → broker_unavailable；closeSession 丢弃会话 stopped 状态", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s7", "ws/lifecycle");
    const stop = await execute(runtime, "stop_computer_control", {}, context);
    assert.equal(stop.isError, false);
    await runtime.dispose();
    const after = await execute(runtime, "list_apps", {}, context);
    assert.equal(after.isError, true);
    assert.equal(JSON.parse(after.content[0].text).code, "broker_unavailable");

    const runtime2 = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context2 = ctx("s7b", "ws/lifecycle2");
    assert.equal((await execute(runtime2, "stop_computer_control", {}, context2)).isError, false);
    await runtime2.closeSession(context2);
    const fresh = await execute(runtime2, "left_click", {}, context2);
    // 会话已删 → 新会话不带 stopped（不能是 controller_busy）。
    assert.notEqual(JSON.parse(fresh.content[0].text).code, "controller_busy");
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 2. 17 码错误装配（逐字锁）

test("错误装配：17 码双落点逐字 + suggested_action 映射 + 码外归 internal", async () => {
  assert.equal(AX_ERROR_CODES.length, 17, "17 码表以 helper/errors.mjs 为源");
  const failures = [
    {
      error: { code: "element_unavailable", message: "element 7 is not available" },
      expectedText: {
        code: "element_unavailable",
        message: "element 7 is not available",
        suggested_action: "Re-observe with get_app_state before acting again.",
      },
      expectedStructured: {
        error: {
          code: "element_unavailable",
          suggested_action: "Re-observe with get_app_state before acting again.",
        },
      },
    },
    {
      error: { code: "foreground_required", message: "target app is not frontmost" },
      expectedText: {
        code: "foreground_required",
        message: "target app is not frontmost",
        suggested_action:
          "Target the element index instead, or bring the app to the foreground first.",
      },
      expectedStructured: {
        error: {
          code: "foreground_required",
          suggested_action:
            "Target the element index instead, or bring the app to the foreground first.",
        },
      },
    },
    {
      // 映射表未覆盖的码 → 省略 suggested_action。
      error: { code: "timeout", message: "broker call timed out" },
      expectedText: { code: "timeout", message: "broker call timed out" },
      expectedStructured: { error: { code: "timeout" } },
    },
    {
      // 码表外 → 归 internal，message 原文保留。
      error: { code: "bogus_code", message: "helper exploded" },
      expectedText: { code: "internal", message: "helper exploded" },
      expectedStructured: { error: { code: "internal" } },
    },
  ];
  let cursor = 0;
  const broker = await startBroker((method) => {
    if (method === "health") return healthOk;
    return { ok: false, error: failures[cursor++].error };
  });
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    for (const failure of failures) {
      const result = await execute(runtime, "list_apps", {}, ctx("s2"));
      assert.equal(result.isError, true);
      assert.equal(result.content.length, 1);
      // 规范级：文本块 = JSON.stringify({code, message, suggested_action})，逐字锁。
      assert.equal(result.content[0].text, JSON.stringify(failure.expectedText));
      assert.deepEqual(result.structuredContent, failure.expectedStructured);
    }
    assert.equal(cursor, failures.length, "每个用例各消耗一次 list_apps 失败");
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 3. 版本比对粘滞

test("health protocolVersion 不符 → version_mismatch 粘滞且只打一次 health", async () => {
  const broker = await startBroker((method) =>
    method === "health"
      ? { ok: true, result: { bundleId: null, pid: 5, protocolVersion: "9.9" } }
      : { ok: true, result: GOOD_APPS },
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const first = await execute(runtime, "list_apps", {}, ctx("s1"));
    assert.equal(first.isError, true);
    const payload = JSON.parse(first.content[0].text);
    assert.equal(payload.code, "version_mismatch");
    // message 必须同时点名 helper 侧与 runtime 侧两个版本。
    assert.ok(payload.message.includes("9.9"), payload.message);
    assert.ok(payload.message.includes(HELPER_PROTOCOL_VERSION), payload.message);
    assert.equal(payload.suggested_action, undefined);
    // 粘滞：跨会话、跨工具（含合成 request_access）一律 version_mismatch。
    const second = await execute(runtime, "request_access", {}, ctx("s2"));
    assert.equal(JSON.parse(second.content[0].text).code, "version_mismatch");
    const third = await execute(runtime, "list_apps", {}, ctx("s1"));
    assert.equal(JSON.parse(third.content[0].text).code, "version_mismatch");
    // runtime 级一次 health，粘滞不再回连。
    assert.equal(broker.calls.filter((call) => call.method === "health").length, 1);
  } finally {
    await broker.close();
  }
});

test("health 成功但缺 protocolVersion → 同样按 version_mismatch 处理", async () => {
  const broker = await startBroker((method) =>
    method === "health" ? { ok: true, result: { bundleId: null, pid: 5 } } : { ok: true, result: GOOD_APPS },
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "list_apps", {}, ctx("s1"));
    assert.equal(result.isError, true);
    assert.equal(JSON.parse(result.content[0].text).code, "version_mismatch");
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 4. 冷启动 CUA_NOT_READY

test("无监听 → 非 error 单文本块 CUA_NOT_READY 信封；broker 起来后同 runtime 重试成功", async () => {
  const socketPath = mintBrokerSocketPath(); // 无人监听
  let ensureCalls = 0;
  const runtime = createComputerUseRuntime({
    brokerSocketPath: socketPath,
    ensureBrokerAvailable: async () => {
      ensureCalls += 1;
      throw new Error("host start failed");
    },
  });
  const context = ctx("cold");
  const result = await execute(runtime, "list_apps", {}, context);
  // 非 error：SDK notReadyEnvelopeOf 对 isError===true 直接返回 undefined。
  assert.notEqual(result.isError, true);
  assert.equal("structuredContent" in result, false, "信封不得带 structuredContent");
  assert.equal(result.content.length, 1);
  // 字段与 SDK 注释钉死的 producer 契约逐字一致。
  assert.deepEqual(JSON.parse(result.content[0].text), {
    kind: "CUA_NOT_READY",
    reasonCode: "broker_not_accepting",
    retryable: true,
    message:
      "The Computer Use helper is starting up. Retry the same tool call after a brief wait.",
  });
  assert.equal(ensureCalls, 1, "冷启动先触发 ensureBrokerAvailable（其失败不得击穿信封）");

  // M2（Task 2 评审 carry）：broker 也纳入 try/finally——前置断言失败不得泄漏监听。
  const broker = await startBroker(defaultHandle, socketPath);
  try {
    assert.equal(broker.socketPath, socketPath, "同一 socket 路径重放");
    // 信封是可重试的：broker 起来后同一 runtime 再调 → 正常成功。
    const retried = await execute(runtime, "list_apps", {}, context);
    assert.deepEqual(JSON.parse(retried.content[0].text), GOOD_APPS);
    assert.equal(ensureCalls, 2);
  } finally {
    await broker.close();
  }
});

test("health 成功后 broker 掉线 → 普通 stale_socket 错误结果（不是冷启动信封）", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("warm");
    const warm = await execute(runtime, "list_apps", {}, context);
    assert.deepEqual(JSON.parse(warm.content[0].text), GOOD_APPS);
    await broker.close();
    const dead = await execute(runtime, "list_apps", {}, context);
    assert.equal(dead.isError, true, "已热身后断连必须是 error 结果");
    assert.equal(JSON.parse(dead.content[0].text).code, "stale_socket");
  } finally {
    await broker.close(); // 已 close 再 close 幂等（未运行态回调即结）
  }
});

// M1（Task 2 评审 carry）：health 预检先于未知工具检查——计划原文「runtime 首次 execute
// 前 health」。冷路径下连未知工具也必须先过健康门：此时返回的是可重试信封而非 method_not_found。
test("M1：预检先于未知工具检查——无监听时未知工具得 CUA_NOT_READY 信封", async () => {
  const runtime = createComputerUseRuntime({ brokerSocketPath: mintBrokerSocketPath() });
  const result = await execute(runtime, "exec", {}, ctx("m1"));
  assert.notEqual(result.isError, true);
  assert.equal(JSON.parse(result.content[0].text).kind, "CUA_NOT_READY");
});

// ────────────────────────────────────────────── 5. stop 闸门与会话隔离

test("stop_computer_control 幂等收据；此后变更类 → controller_busy；非变更类保持开放", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s5", "ws/stop");
    const stop = await execute(runtime, "stop_computer_control", { reason: "done" }, context);
    assert.equal(stop.isError, false);
    assert.deepEqual(JSON.parse(stop.content[0].text), { stopped: true });
    assert.deepEqual(stop.structuredContent, {
      stopped: true,
      action_sent: true,
      dispatch_status: "delivered",
    });
    // 幂等：重复调用同形。
    const again = await execute(runtime, "stop_computer_control", {}, context);
    assert.deepEqual(again, stop);

    const click = await execute(runtime, "left_click", { target: 1 }, context);
    assert.equal(click.isError, true);
    const payload = JSON.parse(click.content[0].text);
    assert.equal(payload.code, "controller_busy");
    // message 明示 stopped（spec §runtime 面）。
    assert.ok(payload.message.includes("computer control was stopped"), payload.message);
    assert.equal(
      payload.suggested_action,
      "Another computer-control session (or a stopped one) owns control; observe state or ask the user.",
    );
    assert.deepEqual(click.structuredContent, {
      error: {
        code: "controller_busy",
        suggested_action:
          "Another computer-control session (or a stopped one) owns control; observe state or ask the user.",
      },
    });

    // 非变更类（list_apps / list_windows / request_access）保持开放。
    assert.equal((await execute(runtime, "list_apps", {}, context)).isError, false);
    assert.equal((await execute(runtime, "list_windows", { app_ref: { pid: 7 } }, context)).isError, false);
    assert.equal((await execute(runtime, "request_access", {}, context)).isError, false);
    // stop 自身可重复（幂等已验），get_app_state 归 Task 3，此处不断言。
  } finally {
    await broker.close();
  }
});

test("会话键隔离：session × workspace 双维 + snake/camel 同槽 + 缺 session 按 workspace 分键", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    // A 会话 stop。
    assert.equal(
      (await execute(runtime, "stop_computer_control", {}, ctx("sA", "ws/1"))).isError,
      false,
    );
    // 同 workspace 不同 session → 独立，不继承 stopped。
    const otherSession = await execute(runtime, "left_click", {}, ctx("sB", "ws/1"));
    assert.notEqual(JSON.parse(otherSession.content[0].text).code, "controller_busy");
    // 同 session 不同 workspace → 独立。
    const otherWorkspace = await execute(runtime, "left_click", {}, ctx("sA", "ws/2"));
    assert.notEqual(JSON.parse(otherWorkspace.content[0].text).code, "controller_busy");

    // brief 会话键的 snake_case 字段与 d.ts camelCase 同槽（同一逻辑会话）。
    const snake = { session_id: "sA", workspace_key: "ws/1", runtimeScope: "main" };
    const viaSnake = await execute(runtime, "left_click", {}, snake);
    assert.equal(JSON.parse(viaSnake.content[0].text).code, "controller_busy");

    // 缺 session_id → 只按 workspace 分键：同 workspace 共享，异 workspace 隔离。
    const noSessionA = { workspaceKey: "ws/only", runtimeScope: "main" };
    const noSessionB = { workspaceKey: "ws/only", runtimeScope: "main" };
    const noSessionOther = { workspaceKey: "ws/elsewhere", runtimeScope: "main" };
    assert.equal((await execute(runtime, "stop_computer_control", {}, noSessionA)).isError, false);
    const shared = await execute(runtime, "left_click", {}, noSessionB);
    assert.equal(JSON.parse(shared.content[0].text).code, "controller_busy");
    const isolated = await execute(runtime, "left_click", {}, noSessionOther);
    assert.notEqual(JSON.parse(isolated.content[0].text).code, "controller_busy");
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 6. 直通工具

test("list_apps：单文本块 = 裸 JSON 数组，无 structuredContent", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "list_apps", {}, ctx("s8"));
    assert.equal(result.isError, false);
    assert.equal(result.content.length, 1);
    assert.equal("structuredContent" in result, false);
    const parsed = JSON.parse(result.content[0].text);
    assert.ok(Array.isArray(parsed), "SDK parseJsonValue 期望裸数组");
    assert.deepEqual(parsed, GOOD_APPS);
  } finally {
    await broker.close();
  }
});

test("list_windows：app_ref 三形态解析 pid + 不可解析归 invalid_request", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("s9");

    // {pid} 直连，不打 list_apps。
    const direct = await execute(runtime, "list_windows", { app_ref: { pid: 7 } }, context);
    assert.deepEqual(JSON.parse(direct.content[0].text), GOOD_WINDOWS);
    assert.equal("structuredContent" in direct, false);

    // {name} 经 list_apps 解析。
    await execute(runtime, "list_windows", { app_ref: { name: "Notepad" } }, context);
    // 裸字符串按 docs 读作 bundle_id。
    await execute(runtime, "list_windows", { app_ref: "notepad.app" }, context);
    const listWindowsCalls = broker.calls.filter((call) => call.method === "list_windows");
    assert.deepEqual(
      listWindowsCalls.map((call) => call.params),
      [{ pid: 7 }, { pid: 7 }, { pid: 7 }],
    );
    const listAppsCalls = broker.calls.filter((call) => call.method === "list_apps");
    assert.equal(listAppsCalls.length, 2, "只有 name/bundle_id 两条走 list_apps 解析");

    // 不可解析 → invalid_request，message 点名 app_ref。
    const missing = await execute(runtime, "list_windows", { app_ref: { name: "Missing App" } }, context);
    assert.equal(missing.isError, true);
    const missingPayload = JSON.parse(missing.content[0].text);
    assert.equal(missingPayload.code, "invalid_request");
    assert.ok(missingPayload.message.includes("Missing App"), missingPayload.message);

    // 缺 app_ref → invalid_request。
    const absent = await execute(runtime, "list_windows", {}, context);
    assert.equal(JSON.parse(absent.content[0].text).code, "invalid_request");
  } finally {
    await broker.close();
  }
});

test("request_access：Windows 合成形态（扁平 AccessStatus 文本 + structuredContent，无 darwin meta）", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "request_access", {}, ctx("s10"));
    assert.equal(result.isError, false);
    assert.equal(
      result.content[0].text,
      JSON.stringify({ ready: true, accessibility: "granted", screenRecording: "granted" }),
    );
    assert.deepEqual(result.structuredContent, {
      platform: "windows",
      backend: "uia",
      accessibility: { status_after: "not_required" },
      screen_recording: { status_after: "not_required" },
    });
    assert.equal("_meta" in result, false, "Windows 不设 darwin-only meta");
    // 不打 broker（除 runtime 级 health 预检外零调用）。
    assert.deepEqual(
      broker.calls.map((call) => call.method),
      ["health"],
    );
  } finally {
    await broker.close();
  }
});

// ────────────────────────────────────────────── 7. 分发表覆盖

// Task 4 注册全部 9 个动作处理器后，「14 名内未注册 → unimplemented」中间态按其注释消失
//（errorResult 保留该码仅剩防御意义）。改为锁新不变量：14 名逐名空参执行，成功或 17 码
// 错误都合法，唯独不得再落 unimplemented——即分发表已全量接线。
test("14 名内工具全部注册：逐名空参执行无 unimplemented（Task 4 后）", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const names = [
      "list_apps",
      "list_windows",
      "get_app_state",
      "left_click",
      "scroll",
      "left_click_drag",
      "type",
      "set_value",
      "select_text",
      "key",
      "perform_action",
      "paste",
      "request_access",
      "stop_computer_control",
    ];
    for (const toolName of names) {
      const result = await execute(runtime, toolName, {}, ctx("s11"));
      if (result.isError !== true) continue;
      const { code } = JSON.parse(result.content[0].text);
      assert.ok(AX_ERROR_CODES.includes(code), `${toolName}: expected one of the 17 codes, got ${code}`);
      assert.notEqual(code, "unimplemented", `${toolName} 仍未注册 handler`);
    }
  } finally {
    await broker.close();
  }
});

test("env 注入的 socket 路径优先于铸造（resolveBrokerSocketPath 兜底）", async () => {
  const broker = await startBroker();
  try {
    const runtime = createComputerUseRuntime({
      env: { MODE_CUA_PERMISSION_BROKER_SOCKET: broker.socketPath },
    });
    const result = await execute(runtime, "list_apps", {}, ctx("s12"));
    assert.deepEqual(JSON.parse(result.content[0].text), GOOD_APPS);
  } finally {
    await broker.close();
  }
});
