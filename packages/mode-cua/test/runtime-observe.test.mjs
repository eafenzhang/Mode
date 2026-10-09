/* eslint-disable max-lines -- 观察面用例按 brief 逐条平铺（渲染正则锁、diff、位移台账、帧对、
   防御分支），断言密集不宜折叠；.oxlintrc 的测试豁免只覆盖 *.test.ts，本计划任务仅允许改本文件。 */
// 观察面契约测试（Plan B Task 3）：get_app_state 的严格四参、窗口解析三路、
// 树文本渲染（UI cuaResultState.ts 逐字正则锁）、diff 语义与计数、位移台账两规则
// （隐藏观察不作基线 + resolveElementIndex 门两向）、帧对顺序/收据/_meta 双键、
// >200KiB 防御错误。fake broker 一律真 net server（行分隔 JSON 往返），
// canned observe/capture 响应按真 addon 实测形状（napi camelCase：windowTitle /
// focusedIndex / enumerationComplete；list_apps bundleId；list_windows windowId；
// capture data 落线为 {type:"Buffer",data:[...]}）。
import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";

import { mintBrokerSocketPath } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import {
  OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY,
  OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
  findOfficialCuaFrameContentPair,
  isOfficialCuaImageRefText,
  readRasterEnvelopeIdentity,
} from "../frame-contract.js";
import { CUA_APP_ASSOCIATIONS_META_KEY } from "../host-display-contract.js";
import { createComputerUseRuntime } from "../index.js";

// ─────────────────────────── UI 解析正则（与 cuaResultState.ts 同款，逐字拷贝）

// 首行 header —— packages/ui/src/ToolCallBlocks/renderers/cuaResultState.ts:28
// （parseCuaTextAppState 的 /^app:\s+([A-Za-z0-9.-]+)\s+pid=\d+\s+"([^"\r\n]+)"\s*$/mu）。
const UI_HEADER_RE = /^app:\s+([A-Za-z0-9.-]+)\s+pid=\d+\s+"([^"\r\n]+)"\s*$/mu;
// 任意元素行 —— cuaResultState.ts:175（readCuaActionTargetName 的 /^\s*\[\d+\]\s+(.+)$/u）。
const UI_ANY_ROW_RE = /^\s*\[\d+\]\s+(.+)$/u;
// 目标行 —— cuaResultState.ts:163（new RegExp(`^\\s*\\[${target.index}\\]\\s+(.+)$`, "u")）。
const uiTargetRowRe = (index) => new RegExp(`^\\s*\\[${index}\\]\\s+(.+)$`, "u");
// 文本子行 —— cuaResultState.ts:178（/^text\s+/iu，连续文本节点才拼名）。
const UI_TEXT_CHILD_RE = /^text\s+/iu;
// 行尾 (…) 剥离 —— cuaResultState.ts:133。
const UI_TRAILING_PARENS_RE = /\s+\([^)]*\)\s*$/u;
// 角色前缀剥离 —— cuaResultState.ts:129（/^\S+\s+/u）。
const UI_ROLE_PREFIX_RE = /^\S+\s+/u;
// 数值/布尔/null 字面量 —— cuaResultState.ts:143。
const UI_LITERAL_RE = /^(?:-?\d+(?:\.\d+)?|true|false|null)$/iu;
// textarea 左侧特判 —— cuaResultState.ts:141。
const UI_TEXTAREA_RE = /^textarea\s+/iu;
// 文本节点目标名 —— cuaResultState.ts:176-182 的子行扫描。
function uiChildNames(lines, targetLineIndex) {
  const names = [];
  for (const line of lines.slice(targetLineIndex + 1)) {
    const childLine = UI_ANY_ROW_RE.exec(line)?.[1];
    if (!childLine || !UI_TEXT_CHILD_RE.test(childLine)) break;
    const name = uiElementName(childLine);
    if (!name) break;
    names.push(name);
  }
  return names;
}
function uiStripRole(value) {
  return value.replace(UI_ROLE_PREFIX_RE, "").trim();
}
// cuaResultState.ts:132-146 readElementName 逐字逻辑（测试侧复刻，防 UI 解析漂移）。
function uiElementName(elementLine) {
  const content = elementLine.replace(UI_TRAILING_PARENS_RE, "").trim();
  const separator = content.lastIndexOf(" = ");
  if (separator < 0) return uiStripRole(content) || null;
  const left = content.slice(0, separator).trim();
  const right = content.slice(separator + 3).trim();
  if (UI_TEXTAREA_RE.test(left)) return uiStripRole(left) || null;
  return UI_LITERAL_RE.test(right) ? uiStripRole(left) || null : right || null;
}

// ────────────────────────────────────────────────────── canned broker 载荷

const GOOD_APPS = [{ pid: 7, name: "Notepad", bundleId: "notepad.app", active: true }];
const GOOD_WINDOWS = [
  {
    windowId: 99,
    pid: 7,
    title: "Untitled",
    bounds: [0, 0, 800, 600],
    main: true,
    focused: false,
    onscreen: true,
  },
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

// 真 addon observe 出参形状（实测 dist-cua-helper/cua_ax.node）。
const observeResult = (elements, extra = {}) => ({
  windowTitle: extra.windowTitle ?? "Untitled",
  focusedIndex: extra.focusedIndex ?? null,
  enumerationComplete: extra.enumerationComplete ?? true,
  elements,
});

// 真 addon capture 落线形状：Buffer → JSON {type:"Buffer",data:[...]}。
const PNG_BYTES = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.alloc(60, 1)]);
const captureResult = (bytes = PNG_BYTES, extra = {}) => ({
  data: { type: "Buffer", data: Array.from(bytes) },
  width: extra.width ?? 64,
  height: extra.height ?? 32,
  clamped: false,
});

const ctx = (sessionId, workspaceKey = "ws/observe") => ({
  sessionId,
  runtimeScope: "main",
  workspaceKey,
  workspacePath: "C:/ws/observe",
});

const healthOk = {
  ok: true,
  result: { bundleId: null, pid: 1234, protocolVersion: HELPER_PROTOCOL_VERSION },
};

// 每个用例自带 handle：observe/capture 载荷按需换，默认回好形状。
function startBroker(handle, socketPath = mintBrokerSocketPath()) {
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

// 默认 handle：health/list_apps/list_windows 恒好；observe/capture 由 overlay 覆盖。
// overlay 值允许是 thunk（`() => payload`）——用例中途换树时按请求时点取值，防「按值捕获」。
function makeHandle(overlay = {}) {
  const resolve = (value) => (typeof value === "function" ? value() : value);
  return (method) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: GOOD_APPS };
    if (method === "list_windows")
      return { ok: true, result: resolve(overlay.windows) ?? GOOD_WINDOWS };
    if (method === "observe")
      return { ok: true, result: resolve(overlay.observe) ?? observeResult([]) };
    if (method === "capture") return { ok: true, result: resolve(overlay.capture) ?? captureResult() };
    return { ok: true, result: {} };
  };
}

const execute = (runtime, toolName, args, context) =>
  runtime.execute({ toolName, arguments: args ?? {}, context });

// 从结果 content 里取「非 JSON」文本块 = 渲染树（SDK appStateOf 同款回退）。
function treeTextOf(result) {
  return result.content.find(
    (block) =>
      block.type === "text" &&
      (() => {
        try {
          JSON.parse(block.text);
          return false;
        } catch {
          return true;
        }
      })(),
  )?.text;
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

// SDK receiptOf 同款合并读法（computer-use-client.mjs:170-203：result 顶层 →
// structuredContent → JSON 文本块，first-defined-wins）——收据必须能从顶层按此路读全。
function sdkReceiptOf(result) {
  const merged = {};
  const sources = [result, result.structuredContent];
  for (const block of result.content ?? []) {
    if (block.type !== "text") continue;
    try {
      sources.push(JSON.parse(block.text));
    } catch {
      // 非 JSON 文本块（渲染树）不参与收据合并。
    }
  }
  for (const source of sources) {
    if (!source || typeof source !== "object") continue;
    for (const key of ["state_id", "frame_id", "snapshot_mode", "base_state_id"]) {
      if (merged[key] === undefined && source[key] !== undefined) merged[key] = source[key];
    }
  }
  return merged;
}

// ───────────────────────────────── 1. 树文本渲染 + UI 正则锁 + idx 透传

test("树文本：header 与元素行逐字匹配 UI 解析正则；idx 原样透传不重编号", async () => {
  // idx 不连续（0,1,7）验证透传：broker 返回什么下标，树里就是什么下标。
  const elements = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { actions: ["press"] }),
    el(7, "edit", "Name", "hi", { offscreen: true }),
    // CUA 把按钮名拆成紧随其后的扁平文本节点：按钮自身只剩数字下标，
    // 目标名由子 text 行拼出（UI readCuaActionTargetName 场景）。
    el(5, "button", "5", null, { actions: ["press"] }),
    el(6, "text", "Save file", null),
  ];
  const broker = await startBroker(makeHandle({ observe: observeResult(elements, { focusedIndex: 7 }) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t1"));
    assert.equal(result.isError, false);
    const tree = treeTextOf(result);
    assert.ok(tree, "带树观察必须有非 JSON 文本块");

    // 首行：UI 正则逐字匹配（Notepad 全 ASCII → sanitize 不变）。
    const header = UI_HEADER_RE.exec(tree);
    assert.ok(header, `header 未匹配 UI 正则: ${JSON.stringify(tree.split("\n")[0])}`);
    assert.equal(header[1], "Notepad");
    assert.match(header[2], /[^\r\n"]+/);
    assert.ok(tree.includes('pid=7 "Untitled"'), tree.split("\n")[0]);

    const lines = tree.split(/\r?\n/u);
    // 全部元素行匹配 UI 任意行正则，且下标集合与 broker 返回一致（无重编号）。
    const rowLines = lines.filter((line) => UI_ANY_ROW_RE.test(line));
    const rowIndexes = rowLines.map((line) => Number(/^\s*\[(\d+)\]/u.exec(line)[1]));
    assert.deepEqual(rowIndexes.sort((a, b) => a - b), [0, 1, 5, 6, 7]);

    // 目标行正则 + readElementName 启发式：数值右侧（null）取左名。
    const okLine = uiTargetRowRe(1).exec(lines.find((line) => line.includes("[1] ")))?.[1];
    assert.equal(uiElementName(okLine), "OK");
    // offscreen 元素行尾 (…) 组可剥；值非字面量（"hi"）时 UI 按启发式取右侧文本。
    const editLine = uiTargetRowRe(7).exec(lines.find((line) => line.includes("[7] ")))?.[1];
    assert.match(editLine, /\(offscreen\)$/u);
    assert.equal(uiElementName(editLine), "hi");
    // actions 行内注记（brief 示例 [3] button OK = null (press) 同形）。
    assert.match(
      lines.find((line) => line.includes("[1] ")),
      /\[1\] button OK = null \(press\)$/u,
    );
    // 数字名按钮 + 紧随 text 子行 → UI 子行扫描拼出目标名。
    const targetIndex = lines.findIndex((line) => uiTargetRowRe(5).test(line));
    const childNames = uiChildNames(lines, targetIndex);
    assert.deepEqual(childNames, ["Save file"]);

    // structuredContent 的 elements 原样带 index（SDK elements() 逃逸口）。
    assert.deepEqual(
      result.structuredContent.elements.map((e) => e.index),
      [0, 1, 7, 5, 6],
    );
    assert.equal(result.structuredContent.focused_element, 7);
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 2. structuredContent 全集 + 收据链

test("structuredContent 字段全集与收据：state_id=uuid、base_state_id 单调、diff 才有 changes", async () => {
  const treeA = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { actions: ["press"] }),
    el(2, "edit", "Name", "hi"),
  ];
  const treeB = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { actions: ["press"], bounds: [20, 20, 80, 24] }),
    el(3, "text", "added row", null),
  ];
  let observed = observeResult(treeA);
  const broker = await startBroker((method) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: GOOD_APPS };
    if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
    if (method === "observe") return { ok: true, result: observed };
    return { ok: true, result: {} };
  });
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const first = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t2"));
    assert.equal(first.isError, false);
    const sc = first.structuredContent;
    // 首观察：全量、无基线。
    assert.match(sc.state_id, UUID_RE, "state_id 必须是 randomUUID");
    assert.equal(sc.base_state_id, null, "首观察无前序 state_id");
    assert.equal(sc.snapshot_mode, "full");
    assert.deepEqual(sc.app, { name: "Notepad", bundle_id: "notepad.app", pid: 7 });
    assert.deepEqual(sc.window, { title: "Untitled", window_id: 99 });
    assert.equal(sc.focused_element, null);
    assert.equal("changes" in sc, false, "全量观察不带 changes");
    assert.equal("frame_id" in sc, false, "无截图不带 frame_id");
    // SDK appStateOf 必需四键齐（state_id/elements/app/window）。
    assert.ok(Array.isArray(sc.elements) && sc.elements.length === 3);
    // 元素归一八键全集。
    assert.deepEqual(Object.keys(sc.elements[1]).sort(), [
      "actions",
      "bounds",
      "enabled",
      "index",
      "kind",
      "offscreen",
      "title",
      "value",
    ]);
    assert.equal("structuredContent" in first && first.isError, false);

    // 第二次观察（树有变）→ diff，base_state_id = 第一次的 state_id。
    observed = observeResult(treeB);
    const second = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t2"));
    assert.equal(second.structuredContent.snapshot_mode, "diff");
    assert.equal(second.structuredContent.base_state_id, sc.state_id);
    assert.notEqual(second.structuredContent.state_id, sc.state_id);
    assert.match(second.structuredContent.state_id, UUID_RE);
    // 收据在 structuredContent 顶层：按 SDK receiptOf 的合并读法（client :170-203）应读全。
    const receipt = sdkReceiptOf(second);
    assert.equal(receipt.state_id, second.structuredContent.state_id);
    assert.equal(receipt.base_state_id, sc.state_id);
    assert.equal(receipt.snapshot_mode, "diff");
    // diff 下 elements 仍是当前全表（SDK elements() 语义），changes 另列。
    assert.equal(second.structuredContent.elements.length, 3);
    assert.deepEqual(second.structuredContent.changes.added_count, 1);
    assert.equal(second.structuredContent.changes.removed_count, 1);
    assert.equal(second.structuredContent.changes.changed_count, 1);
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 3. strict 四参

test("strict 四参：未知键 / 缺 app_ref / 非布尔 / 非法 window_id → invalid_request", async () => {
  const broker = await startBroker(makeHandle({ observe: observeResult([]) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t3");
    const unknown = await execute(runtime, "get_app_state", { app_ref: { pid: 7 }, extra: 1 }, context);
    assert.equal(unknown.isError, true);
    const unknownPayload = JSON.parse(unknown.content[0].text);
    assert.equal(unknownPayload.code, "invalid_request");
    assert.ok(unknownPayload.message.includes("extra"), unknownPayload.message);

    const missing = await execute(runtime, "get_app_state", {}, context);
    assert.equal(JSON.parse(missing.content[0].text).code, "invalid_request");
    assert.ok(JSON.parse(missing.content[0].text).message.includes("app_ref"));

    const badBool = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: "yes" },
      context,
    );
    assert.equal(JSON.parse(badBool.content[0].text).code, "invalid_request");
    assert.ok(JSON.parse(badBool.content[0].text).message.includes("include_screenshot"));

    const badWindow = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7, window_id: "123" } },
      context,
    );
    assert.equal(JSON.parse(badWindow.content[0].text).code, "invalid_request");
    assert.ok(JSON.parse(badWindow.content[0].text).message.includes("window_id"));
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 4. 窗口解析三路

test("窗口解析三路：显式 window_id 不打 list_windows；缺省取 main||focused 首行；无匹配 → invalid_request 点名", async () => {
  // (a) 显式绑定。
  const brokerA = await startBroker(makeHandle({ observe: observeResult([]) }));
  // (b)(c) 自定义窗口行。
  const brokerB = await startBroker(
    makeHandle({
      windows: [
        { windowId: 50, pid: 7, title: "Tool", main: false, focused: false },
        { windowId: 70, pid: 7, title: "Pane", main: false, focused: true },
      ],
      observe: observeResult([]),
    }),
  );
  const brokerC = await startBroker(
    makeHandle({
      windows: [{ windowId: 60, pid: 7, title: "Tray", main: false, focused: false }],
      observe: observeResult([]),
    }),
  );
  try {
    const runtimeA = createComputerUseRuntime({ brokerSocketPath: brokerA.socketPath });
    const explicit = await execute(
      runtimeA,
      "get_app_state",
      { app_ref: { pid: 7, window_id: 123 } },
      ctx("t4"),
    );
    assert.equal(explicit.isError, false);
    assert.deepEqual(
      brokerA.calls.filter((c) => c.method === "list_windows"),
      [],
      "显式 window_id 不得回连 list_windows",
    );
    assert.deepEqual(
      brokerA.calls.filter((c) => c.method === "observe").map((c) => c.params),
      [{ windowId: 123 }],
    );
    assert.equal(explicit.structuredContent.window.window_id, 123);

    // (b) 缺省 → list_windows(pid) 取 main||focused 首行（无 main 行时取 focused 行）。
    const runtimeB = createComputerUseRuntime({ brokerSocketPath: brokerB.socketPath });
    const resolved = await execute(runtimeB, "get_app_state", { app_ref: { pid: 7 } }, ctx("t4b"));
    assert.equal(resolved.isError, false);
    assert.deepEqual(
      brokerB.calls.filter((c) => c.method === "list_windows").map((c) => c.params),
      [{ pid: 7 }],
    );
    assert.deepEqual(
      brokerB.calls.filter((c) => c.method === "observe").map((c) => c.params),
      [{ windowId: 70 }],
    );
    assert.equal(resolved.structuredContent.window.window_id, 70);

    // (c) 无 main/focused 行 → invalid_request，message 点名。
    const runtimeC = createComputerUseRuntime({ brokerSocketPath: brokerC.socketPath });
    const none = await execute(runtimeC, "get_app_state", { app_ref: { pid: 7 } }, ctx("t4c"));
    assert.equal(none.isError, true);
    const nonePayload = JSON.parse(none.content[0].text);
    assert.equal(nonePayload.code, "invalid_request");
    assert.ok(/window/iu.test(nonePayload.message), nonePayload.message);
    assert.deepEqual(none.structuredContent, {
      error: { code: "invalid_request" },
    });
  } finally {
    await brokerA.close();
    await brokerB.close();
    await brokerC.close();
  }
});

// ───────────────────────────────── 5. 内容顺序（带树、无截图）

test("带树无截图：content = 单个 non-JSON text 块，且无 _meta", async () => {
  const broker = await startBroker(
    makeHandle({ observe: observeResult([el(0, "window", "Untitled", null)]) }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t5"));
    assert.equal(result.isError, false);
    assert.equal(result.content.length, 1);
    assert.equal(result.content[0].type, "text");
    assert.throws(() => JSON.parse(result.content[0].text), "树文本必须是 non-JSON（advisory 过滤依赖）");
    assert.equal("_meta" in result, false, "无截图不得带 _meta");
    assert.equal(result.structuredContent.snapshot_mode, "full");
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 6. 纯截图观察：无树 + 下一次带树强制全量

test("纯截图观察：content=[image@0, ref@1] 无树文本；下一次带树观察强制全量", async () => {
  const treeA = [el(0, "window", "Untitled", null), el(1, "button", "OK", null)];
  const treeB = [el(0, "window", "Untitled", null), el(2, "button", "Go", null)];
  let observed = observeResult(treeA);
  const broker = await startBroker(makeHandle({ observe: () => observed }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t6");

    // 第一次带树观察（shown）→ 建立基线。
    const first = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.equal(first.structuredContent.snapshot_mode, "full");

    // 纯截图观察（include_screenshot && tree_shown_to_model=false）。
    observed = observeResult(treeB);
    const shot = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: true, tree_shown_to_model: false },
      context,
    );
    assert.equal(shot.isError, false);
    assert.equal(shot.content.length, 2, "纯截图观察不得带树文本块");
    assert.equal(shot.content[0].type, "image");
    assert.equal(shot.content[1].type, "text");
    assert.ok(isOfficialCuaImageRefText(shot.content[1].text));
    assert.equal(treeTextOf(shot), undefined, "纯截图观察无渲染树");
    // 无树观察仍须带结构化面（SDK appStateOf 需要 state_id/elements/app/window）。
    assert.match(shot.structuredContent.state_id, UUID_RE);
    assert.equal(shot.structuredContent.elements.length, 2);

    // 截图后第一次带树观察：强制全量（docs：screenshot-only 之后必须先拿整树）。
    const after = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.equal(after.structuredContent.snapshot_mode, "full", "截图后首观察必须全量");
    assert.equal("changes" in after.structuredContent, false);

    // 强制标志消费后，下一次带树观察回到正常 diff 轨道（基线 = 上一次 shown）。
    observed = observeResult([...treeB, el(4, "text", "more", null)]);
    const diffed = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.equal(diffed.structuredContent.snapshot_mode, "diff");
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 7. diff 计数与树面

test("diff：changes 计数 + 树只列变更行 + elements 保持当前全表", async () => {
  const treeA = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { actions: ["press"] }),
    el(2, "edit", "Name", null),
  ];
  const treeB = [
    el(0, "window", "Untitled", null), // 不变 → 不入树
    el(1, "button", "OK", null, { actions: ["press"], bounds: [20, 20, 80, 24] }), // changed
    el(3, "text", "fresh", null), // added
  ];
  let observed = observeResult(treeA);
  const broker = await startBroker(makeHandle({ observe: () => observed }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t7");
    await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);

    observed = observeResult(treeB);
    const diffed = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    const sc = diffed.structuredContent;
    assert.equal(sc.snapshot_mode, "diff");
    assert.equal(sc.changes.added_count, 1);
    assert.equal(sc.changes.removed_count, 1);
    assert.equal(sc.changes.changed_count, 1);
    assert.deepEqual(
      sc.changes.added.map((e) => e.index),
      [3],
    );
    assert.deepEqual(
      sc.changes.removed.map((e) => e.index),
      [2],
    );
    assert.deepEqual(
      sc.changes.changed.map((e) => e.index),
      [1],
    );
    // elements = 当前全表（3 个现役元素，removed 的 2 不在其中）。
    assert.deepEqual(
      sc.elements.map((e) => e.index),
      [0, 1, 3],
    );

    const tree = treeTextOf(diffed);
    const lines = tree.split(/\r?\n/u);
    assert.match(lines[0], UI_HEADER_RE);
    assert.ok(lines.includes("changes: +1 -1 ~1"), tree);
    const rowLines = lines.filter((line) => UI_ANY_ROW_RE.test(line));
    const rowIndexes = rowLines.map((line) => Number(/^\s*\[(\d+)\]/u.exec(line)[1]));
    assert.deepEqual(rowIndexes.sort((a, b) => a - b), [1, 2, 3], "只列 added/changed/removed");
    assert.equal(rowIndexes.includes(0), false, "未变更行不得入树（省略行下标仍有效）");
    // removed 行带 (removed) 注记、可被 UI 剥离解析。
    const removedLine = rowLines.find((line) => line.startsWith("[2] "));
    assert.match(removedLine, /\(removed\)$/u);
    assert.equal(uiElementName(removedLine.replace(/^\[\d+\]\s+/u, "")), "Name");
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 8. 位移（index 消失但指纹他处）

test("diff 位移：index 消失但指纹在别处 → 计 changed，不计 added/removed", async () => {
  const okBounds = [10, 0, 80, 24];
  const treeA = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { bounds: okBounds }),
  ];
  // 同一按钮换到下标 2（kind/title/value/bounds 指纹完全一致）。
  const treeB = [
    el(0, "window", "Untitled", null),
    el(2, "button", "OK", null, { bounds: okBounds }),
  ];
  let observed = observeResult(treeA);
  const broker = await startBroker(makeHandle({ observe: () => observed }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t8");
    await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    observed = observeResult(treeB);
    const diffed = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    const sc = diffed.structuredContent;
    assert.equal(sc.snapshot_mode, "diff");
    assert.equal(sc.changes.changed_count, 1, "位移只算一次 changed");
    assert.equal(sc.changes.added_count, 0, "指纹匹配到旧下标 → 不算 added");
    assert.equal(sc.changes.removed_count, 0, "指纹匹配到新下标 → 不算 removed");
    assert.deepEqual(
      sc.changes.changed.map((e) => e.index),
      [2],
      "changed 记录落在新下标",
    );
    assert.ok(treeTextOf(diffed).includes("changes: +0 -0 ~1"));
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 9. 位移台账门（两向）

test("位移台账：隐藏观察重编号 → resolveElementIndex 拒；隐藏与基线一致 → 放行返回 bounds", async () => {
  const g1 = [
    el(0, "window", "Untitled", null),
    el(1, "button", "OK", null, { bounds: [10, 10, 80, 24] }),
  ];
  // 隐藏观察里元素重排/位移（模型没看过这棵树）。
  const g2 = [
    el(0, "button", "OK", null, { bounds: [99, 99, 80, 24] }),
    el(1, "window", "Untitled", null),
  ];
  let observed = observeResult(g1);
  const broker = await startBroker(makeHandle({ observe: () => observed }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t9");
    const key = "99"; // appRefKey = 解析出的 window id（String）

    // 未观察 → 拒。
    assert.throws(
      () => runtime.resolveElementIndex(context, key, 0),
      (error) => error.code === "element_unavailable",
    );

    // 观察过但从未 shown（先来一次隐藏观察）→ 同码拒，文案指 never shown（评审 M4）。
    observed = observeResult(g1);
    await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, tree_shown_to_model: false },
      context,
    );
    assert.throws(
      () => runtime.resolveElementIndex(context, key, 0),
      (error) => {
        assert.equal(error.code, "element_unavailable");
        assert.match(error.message, /never been shown to the model/u);
        return true;
      },
    );

    // shown 基线观察 → 放行，返回 bounds。
    await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.deepEqual(runtime.resolveElementIndex(context, key, 1), [10, 10, 80, 24]);

    // 隐藏观察（元素表变了）→ 规则 2：序列 ≠ 最近 shown 基线 → 拒，提示 re-observe。
    observed = observeResult(g2);
    await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, tree_shown_to_model: false },
      context,
    );
    assert.throws(
      () => runtime.resolveElementIndex(context, key, 0),
      (error) => {
        assert.equal(error.code, "element_unavailable");
        assert.match(error.message, /get_app_state/u, "拒绝文案必须指回重新观察");
        return true;
      },
    );
    // 下标不存在同样拒（元素表里没有该 index）。
    assert.throws(
      () => runtime.resolveElementIndex(context, key, 42),
      (error) => error.code === "element_unavailable",
    );

    // 隐藏观察但元素序列与最近 shown 基线逐位一致 → 放行（规则 2 的反向）。
    observed = observeResult(g1);
    await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, tree_shown_to_model: false },
      context,
    );
    assert.deepEqual(runtime.resolveElementIndex(context, key, 1), [10, 10, 80, 24]);
    // 序列一致时即便下标在「隐藏观察」里也放行；不存在的下标仍拒。
    assert.throws(
      () => runtime.resolveElementIndex(context, key, 77),
      (error) => error.code === "element_unavailable",
    );
    // 门直查不破坏 runtime：后续观察照常成功（state_id 为 string）。
    const after = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.equal(typeof after.structuredContent.state_id, "string");
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 10. 帧对 + meta + 收据

test("帧对：[image@0, ref@1, tree@2] + integrity meta 真值 + app-associations + frame_id 收据", async () => {
  const broker = await startBroker(
    makeHandle({ observe: observeResult([el(0, "window", "Untitled", null)]) }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: true },
      ctx("t10"),
    );
    assert.equal(result.isError, false);
    assert.equal(result.content.length, 3);
    assert.equal(result.content[0].type, "image");
    assert.equal(result.content[1].type, "text");
    assert.equal(result.content[2].type, "text");
    // 帧对强校验：image@0 + ref@1（投影层要求恰在此位）。
    const pair = findOfficialCuaFrameContentPair(result.content);
    assert.equal(pair.imageIndex, 0);
    assert.equal(pair.imageRefIndex, 1);
    assert.ok(isOfficialCuaImageRefText(result.content[1].text));

    const ref = JSON.parse(result.content[1].text).image_ref;
    // 收据与 ref/meta 三方同 frame_id；digest 以 ref/真实栅格为准（绝不读 meta 判定）。
    assert.equal(result.structuredContent.frame_id, ref.frame_id);
    const meta = result._meta[OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY];
    assert.equal(meta.v, 1);
    assert.equal(meta.frame_id, ref.frame_id);
    const digest = readRasterEnvelopeIdentity(result.content[0]);
    assert.equal(ref.raster_sha256, digest, "ref 的 raster_sha256 必须是真实字节 digest");
    assert.equal(meta.raster_sha256, digest);
    assert.equal(ref.mimeType, "image/png");
    assert.equal(ref.width, 64);
    assert.equal(ref.height, 32);
    // capture 实参形状钉死：helper 侧 requireU32(params.windowId) 硬契约（helper/server.mjs）。
    assert.deepEqual(
      broker.calls.filter((c) => c.method === "capture").map((c) => c.params),
      [{ windowId: 99 }],
    );
    // base64 ≤ 200KiB 防线上限。
    assert.ok(
      Buffer.byteLength(result.content[0].data, "utf8") <= OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
    );
    // 树在帧对之后（@2），仍是单个 non-JSON 文本块。
    assert.ok(treeTextOf(result));
    // app-associations：appKey = bundle_id ?? name ?? String(pid)，displayName = 原始名。
    const associations = result._meta[CUA_APP_ASSOCIATIONS_META_KEY];
    assert.deepEqual(associations, {
      primary: { appKey: "notepad.app", displayName: "Notepad" },
    });
    // 观察记录落 session（帧账本供 Task 4 坐标目标用）；收据 frame_id 走顶层可读。
    assert.equal(sdkReceiptOf(result).frame_id, ref.frame_id);
  } finally {
    await broker.close();
  }
});

test("app-associations appKey 回退链：无 bundleId → name；皆无 → String(pid)", async () => {
  const noBundle = [{ pid: 8, name: "calc.exe", bundleId: null, active: true }];
  const bare = [{ pid: 9, name: null, bundleId: null, active: true }];
  for (const [apps, expectedKey, expectedName] of [
    [noBundle, "calc.exe", "calc.exe"],
    [bare, "9", "9"],
  ]) {
    const broker = await startBroker((method) => {
      if (method === "health") return healthOk;
      if (method === "list_apps") return { ok: true, result: apps };
      if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
      if (method === "observe") return { ok: true, result: observeResult([]) };
      if (method === "capture") return { ok: true, result: captureResult() };
      return { ok: true, result: {} };
    });
    try {
      const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
      const result = await execute(
        runtime,
        "get_app_state",
        { app_ref: { pid: apps[0].pid }, include_screenshot: true },
        ctx(`t10b-${apps[0].pid}`),
      );
      assert.deepEqual(result._meta[CUA_APP_ASSOCIATIONS_META_KEY], {
        primary: { appKey: expectedKey, displayName: expectedName },
      });
      assert.equal(result.structuredContent.app.name, expectedName);
    } finally {
      await broker.close();
    }
  }
});

test("app-associations 超 16KiB → 弃用该键，integrity meta 保留", async () => {
  const hugeName = "N".repeat(20_000);
  const broker = await startBroker((method) => {
    if (method === "health") return healthOk;
    if (method === "list_apps")
      return { ok: true, result: [{ pid: 7, name: hugeName, bundleId: null, active: true }] };
    if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
    if (method === "observe") return { ok: true, result: observeResult([]) };
    if (method === "capture") return { ok: true, result: captureResult() };
    return { ok: true, result: {} };
  });
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: true },
      ctx("t10c"),
    );
    assert.equal(result.isError, false);
    assert.equal(CUA_APP_ASSOCIATIONS_META_KEY in result._meta, false, "16KiB 上限外弃用 associations");
    assert.ok(OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY in result._meta, "integrity meta 不受牵连");
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 11. >200KiB 防御

test("capture base64 > 200KiB → internal 错误（防御分支，不发超限帧）", async () => {
  const oversized = "A".repeat(OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES + 1);
  const broker = await startBroker(
    makeHandle({
      observe: observeResult([el(0, "window", "Untitled", null)]),
      capture: { data: oversized, width: 64, height: 32, clamped: false },
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: true },
      ctx("t11"),
    );
    assert.equal(result.isError, true);
    const payload = JSON.parse(result.content[0].text);
    assert.equal(payload.code, "internal");
    assert.match(payload.message, /200KiB/u);
    assert.deepEqual(result.structuredContent, { error: { code: "internal" } });
    // 绝不发出超限帧（content 里无 image/ref 对）。
    assert.equal(findOfficialCuaFrameContentPair(result.content), undefined);
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 12. 标题与焦点位

test("window.title/focused_element 取 observe 实测（windowTitle/focusedIndex），缺省回落", async () => {
  const broker = await startBroker(
    makeHandle({
      observe: observeResult([el(0, "pane", null, null)], {
        windowTitle: "",
        focusedIndex: undefined,
      }),
    }),
  );
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t12"));
    assert.equal(result.isError, false);
    // observe 的 windowTitle 为空 → 回落 list_windows 行标题。
    assert.deepEqual(result.structuredContent.window, { title: "Untitled", window_id: 99 });
    assert.equal(result.structuredContent.focused_element, null);
    // 头部 quoted 段非空（UI 正则要求 [^"\r\n]+）。
    assert.ok(UI_HEADER_RE.test(treeTextOf(result)), treeTextOf(result).split("\n")[0]);
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 13. 隐藏无截图观察（bindApp 形态）

test("隐藏无截图观察：content 空、全量收据、不动 diff 基线", async () => {
  const treeA = [el(0, "window", "Untitled", null), el(1, "button", "OK", null)];
  const treeB = [el(0, "window", "Untitled", null), el(2, "button", "Go", null)];
  let observed = observeResult(treeA);
  const broker = await startBroker(makeHandle({ observe: () => observed }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t13");

    // shown 基线。
    await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);

    // 隐藏观察（SDK bindApp / elements() 形态：disable_diffing + shown=false）。
    observed = observeResult(treeB);
    const hidden = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, disable_diffing: true, tree_shown_to_model: false },
      context,
    );
    assert.equal(hidden.isError, false);
    assert.deepEqual(hidden.content, [], "隐藏观察不得产出模型可见内容");
    assert.equal(hidden.structuredContent.snapshot_mode, "full");
    assert.equal(hidden.structuredContent.elements.length, 2, "隐藏观察更新元素表");

    // 下一次带树观察仍相对最近 **shown** 基线（treeA）diff——隐藏观察既非基线也不强制全量。
    observed = observeResult([...treeB, el(3, "text", "x", null)]);
    const shown = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, context);
    assert.equal(shown.structuredContent.snapshot_mode, "diff", "隐藏观察不改变基线轨道");
    assert.equal(shown.structuredContent.base_state_id, hidden.structuredContent.state_id, "base 链单调");
    // diff 相对 treeA：treeA 里的 [1] 已消失、[2]/[3] 新增。
    const changes = shown.structuredContent.changes;
    assert.equal(changes.removed_count, 1);
    assert.equal(changes.added_count, 2);
    assert.equal(changes.changed_count, 0);
  } finally {
    await broker.close();
  }
});

// ───────────────────────────────── 14. 稀疏 / 截断提示行

test("提示行：enumerationComplete=false → indices are sparse；元素达 3000 → showing A-B of N", async () => {
  const sparseElements = [el(0, "window", "Untitled", null)];
  const cappedElements = Array.from({ length: 3000 }, (_, index) =>
    el(index, "text", `t${index}`, null, { bounds: [0, 0, 1, 1] }),
  );

  // 稀疏：截断标志。
  const sparseBroker = await startBroker(
    makeHandle({
      observe: observeResult(sparseElements, { enumerationComplete: false }),
    }),
  );
  // 截顶：3000 元素且枚举完整（total 恰等于上限）。
  const cappedBroker = await startBroker(
    makeHandle({ observe: observeResult(cappedElements) }),
  );
  try {
    const sparseRuntime = createComputerUseRuntime({ brokerSocketPath: sparseBroker.socketPath });
    const sparse = await execute(sparseRuntime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t14"));
    const sparseTree = treeTextOf(sparse);
    assert.ok(sparseTree.includes("indices are sparse"), sparseTree);
    assert.equal(sparseTree.includes("showing "), false);

    const cappedRuntime = createComputerUseRuntime({ brokerSocketPath: cappedBroker.socketPath });
    const capped = await execute(cappedRuntime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t14b"));
    const cappedTree = treeTextOf(capped);
    assert.ok(cappedTree.includes("showing 1-3000 of 3000 items"), cappedTree.slice(0, 200));
    assert.equal(cappedTree.includes("indices are sparse"), false, "枚举完整 ≠ 稀疏");
    assert.equal(capped.structuredContent.elements.length, 3000);
    // 提示行之后仍是可解析的元素行（UI 行正则不受提示行影响）。
    const rowLines = cappedTree.split(/\r?\n/u).filter((line) => UI_ANY_ROW_RE.test(line));
    assert.equal(rowLines.length, 3000);
  } finally {
    await sparseBroker.close();
    await cappedBroker.close();
  }
});

// ───────────────────────────────── 15. 评审 fix round 1（I1 + M3）

test("单注记组：actions+offscreen 合并为一个 (…) 组，UI 剥离后目标名仍正确", async () => {
  const elements = [
    el(0, "window", "Untitled", null),
    el(4, "button", "OK", null, { actions: ["press"], offscreen: true }),
  ];
  const broker = await startBroker(makeHandle({ observe: observeResult(elements) }));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(runtime, "get_app_state", { app_ref: { pid: 7 } }, ctx("t15"));
    const tree = treeTextOf(result);
    // 单组渲染（两个组会让 UI 只剥末组、右侧变 "null (press)"，目标名失真——§1.2 论证的锁）。
    assert.match(tree, /\[4\] button OK = null \(press, offscreen\)$/mu);
    const line = uiTargetRowRe(4).exec(tree.split(/\r?\n/u).find((l) => l.includes("[4] ")))?.[1];
    assert.equal(uiElementName(line), "OK", "剥离单组后右侧仍是字面量 null → 取左名");
  } finally {
    await broker.close();
  }
});

test("app_ref bundle_id 解析：真 wire 驼峰 bundleId 行与历史 snake 行都可解析；无匹配 → 透明启动后 launch_failed", async () => {
  const rows = [
    { pid: 11, name: "Alpha", bundleId: "com.example.alpha", active: true },
    { pid: 12, name: "Beta", bundle_id: "com.example.beta", active: true },
  ];
  const broker = await startBroker((method) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: rows };
    if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
    if (method === "observe") return { ok: true, result: observeResult([]) };
    return { ok: true, result: {} };
  });
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const context = ctx("t16");

    // 驼峰行（真 wire 形状）解析 {bundle_id} app_ref → 成功，身份回填同键。
    const camel = await execute(
      runtime,
      "get_app_state",
      { app_ref: { bundle_id: "com.example.alpha" } },
      context,
    );
    assert.equal(camel.isError, false, JSON.stringify(camel.content));
    assert.deepEqual(camel.structuredContent.app, {
      name: "Alpha",
      pid: 11,
      bundle_id: "com.example.alpha",
    });

    // snake 行（历史 fake broker 形状）保持可用。
    const snake = await execute(
      runtime,
      "get_app_state",
      { app_ref: { bundle_id: "com.example.beta" } },
      context,
    );
    assert.equal(snake.isError, false, JSON.stringify(snake.content));
    assert.equal(snake.structuredContent.app.pid, 12);
    assert.equal(snake.structuredContent.app.bundle_id, "com.example.beta");

    // 无匹配 → 透明启动（终审 I1）：launch_app 后轮询界内仍无行 → launch_failed；
    // message 保留 SDK isAppNotFound 判定短语
    //（computer-use-client.mjs:410-415 /target app is not running/，bindApp 依此触发 alternateAppRef 重试）。
    const missing = await execute(
      runtime,
      "get_app_state",
      { app_ref: { bundle_id: "com.example.absent" } },
      context,
    );
    assert.equal(missing.isError, true);
    const payload = JSON.parse(missing.content[0].text);
    assert.equal(payload.code, "launch_failed");
    assert.ok(payload.message.includes("target app is not running"), payload.message);
    assert.ok(payload.message.includes("com.example.absent"), payload.message);
    assert.equal(payload.message.includes("did not start"), true, payload.message);
    // 键名按 resolved 字段（bundle_id → helper backend 的 bundleId）。
    assert.deepEqual(
      broker.calls.filter((c) => c.method === "launch_app").map((c) => c.params),
      [{ bundleId: "com.example.absent" }],
    );
  } finally {
    await broker.close();
  }
});

// 终审 I5（spec 风险行 owner = Plan B）：capture 前 screen_probe 预检——锁屏 →
// permission_denied（17 码无 screen_locked），零 capture 下发。
test("锁屏预检：screen_probe {locked:true} → permission_denied，零 capture（终审 I5）", async () => {
  const broker = await startBroker((method) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: GOOD_APPS };
    if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
    if (method === "observe") return { ok: true, result: observeResult([]) };
    if (method === "screen_probe") return { ok: true, result: { locked: true } };
    if (method === "capture") return { ok: true, result: captureResult() };
    return { ok: true, result: {} };
  });
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    const result = await execute(
      runtime,
      "get_app_state",
      { app_ref: { pid: 7 }, include_screenshot: true },
      ctx("t17"),
    );
    assert.equal(result.isError, true, JSON.stringify(result).slice(0, 300));
    const payload = JSON.parse(result.content[0].text);
    assert.equal(payload.code, "permission_denied");
    assert.match(payload.message, /locked/u, payload.message);
    // 预检发生在 capture 之前：锁屏时零 capture，且 probe 恰一次。
    assert.equal(broker.calls.filter((c) => c.method === "capture").length, 0);
    assert.equal(broker.calls.filter((c) => c.method === "screen_probe").length, 1);
    // 未锁屏（probe.locked !== true）→ 照常出帧。
    const unlocked = await startBroker((method) => {
      if (method === "health") return healthOk;
      if (method === "list_apps") return { ok: true, result: GOOD_APPS };
      if (method === "list_windows") return { ok: true, result: GOOD_WINDOWS };
      if (method === "observe") return { ok: true, result: observeResult([]) };
      if (method === "screen_probe") return { ok: true, result: { locked: false } };
      if (method === "capture") return { ok: true, result: captureResult() };
      return { ok: true, result: {} };
    });
    try {
      const openRuntime = createComputerUseRuntime({ brokerSocketPath: unlocked.socketPath });
      const shot = await execute(
        openRuntime,
        "get_app_state",
        { app_ref: { pid: 7 }, include_screenshot: true },
        ctx("t17b"),
      );
      assert.equal(shot.isError, false, JSON.stringify(shot).slice(0, 300));
      assert.equal(unlocked.calls.filter((c) => c.method === "capture").length, 1);
    } finally {
      await unlocked.close();
    }
  } finally {
    await broker.close();
  }
});
