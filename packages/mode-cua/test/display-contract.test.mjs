/* eslint-disable max-lines -- display 消费方契约替身 + canned 场景断言平铺（Task 7 Part A），
   副本/正则与逐行来源注释占多数；与同目录 runtime-*.test.mjs 的 max-lines 处置一致。 */
// display 黄金 · 消费方契约替身（Plan B Task 7 Part A）。
//
// 落点探测结论（决定走本文件，证据见 task-7-report.md §Part A）：
//   packages/services 不能 import "@mode/core" —— ① services package.json 无该依赖，
//   node_modules/@mode 只链 cua/provider/provider-node/rpc/shared；② tsconfig.base 与
//   services/tsconfig 均无 paths/别名；③ 既有 services 测试零先例，全仓无任何带 test
//   脚本的包测过 createCuaToolResultDisplay；④ 运行时探测 import("@mode/core") →
//   ERR_MODULE_NOT_FOUND（core dist 还被 apps/mode-cli/.gitignore 忽略，fresh checkout 不可用）。
//   → 按 brief 规则降级为本包内的消费方契约替身；真正的
//   createCuaToolResultDisplay → toolResultDisplaySchema.parse(strict) 跨包黄金
//   **显式移交 Plan C 的 desktop E2E**（本文件断言 runtime 出参逐字可被现有消费方消费，
//   但 display 投影本身是逐行等价副本，非 core 真身）。
//
// 副本来源（每处标注文件:行号）：
//   - display 投影：core result-display.ts:249-331（createCuaToolResultDisplay）
//   - strict 形状：shared mode-protocol-v4/toolDisplay.ts:67-131（kind:"cua" 成员，strict）
//   - 帧权威胶水：core mcp/image-normalization.ts:92-115（hasOfficialCuaFrameAuthority）
//   - media 发现：ui cuaScreenshotDetails.ts:20-49、68
//   - 树/列表解析：ui cuaResultState.ts:28、163、94-105
//   - targetApp：contracts tool-result-metadata.ts:23（键）、38-46（strict schema）
// 真身直用（零跨包）：frame-contract.js 的 isOfficialCuaImageRefText /
// OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY / OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES、
// request-access-contract.js 的 CUA_REQUEST_ACCESS_STATUS_META_KEY 与 schema
//（core result-display.ts:31-35 从本包 import 同一模块，权限位行为与 core 逐字同源）。
import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";

import { mintBrokerSocketPath } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import {
  OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY,
  OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
  isOfficialCuaImageRefText,
} from "../frame-contract.js";
import {
  CUA_REQUEST_ACCESS_STATUS_META_KEY,
  cuaRequestAccessStatusSchema,
} from "../request-access-contract.js";
import { AX_ERROR_CODES } from "../helper/errors.mjs";
import { createComputerUseRuntime } from "../index.js";

// ────────────────────────────────────────────── canned broker（与 runtime-action 同款真 net 序列化）

const GOOD_APPS = [
  { pid: 7, name: "Notepad", bundleId: "notepad.app", active: true },
  { pid: 8, name: "calc.exe", bundleId: null, active: false },
];
const ALL_WINDOWS = [
  { windowId: 99, pid: 7, title: "Untitled", bounds: [0, 0, 800, 600], main: true, focused: true, onscreen: true },
  { windowId: 77, pid: 8, title: "Calculator", bounds: [0, 0, 300, 200], main: true, focused: false, onscreen: true },
];
const ctx = (sessionId) => ({
  sessionId,
  runtimeScope: "main",
  workspaceKey: "ws/display",
  workspacePath: "C:/ws/display",
});
const healthOk = {
  ok: true,
  result: { bundleId: null, pid: 1234, protocolVersion: HELPER_PROTOCOL_VERSION },
};
const el = (index, kind, title, value, extra = {}) => ({
  index,
  kind,
  title,
  value,
  bounds: extra.bounds ?? [index * 10, 0, 80, 24],
  actions: extra.actions ?? [],
  enabled: true,
  offscreen: false,
});
const observeResult = (elements) => ({
  windowTitle: "Untitled",
  focusedIndex: null,
  enumerationComplete: true,
  elements,
});
// 64x32 小 PNG 字节：buildFrame 只要非空 base64 + 合法尺寸即可出帧。
const PNG_BYTES = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.alloc(60, 1)]);
const captureResult = () => ({
  data: { type: "Buffer", data: Array.from(PNG_BYTES) },
  width: 64,
  height: 32,
  clamped: false,
});

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
      resolve({ socketPath, calls, close: () => new Promise((done) => server.close(done)) }),
    );
  });
}

function makeHandle(overlay = {}) {
  const resolve = (value, params) => (typeof value === "function" ? value(params) : value);
  return (method, params) => {
    if (method === "health") return healthOk;
    if (method === "list_apps") return { ok: true, result: GOOD_APPS };
    if (method === "list_windows") {
      return {
        ok: true,
        result: ALL_WINDOWS.filter((row) => row.pid === params.pid),
      };
    }
    if (method === "observe") return { ok: true, result: resolve(overlay.observe, params) ?? observeResult([]) };
    if (method === "capture") return { ok: true, result: resolve(overlay.capture, params) ?? captureResult() };
    if (method === "perform") {
      const responder = overlay.perform ?? { ok: true, result: { dispatched: "dispatched" } };
      return resolve(responder, params);
    }
    return { ok: true, result: {} };
  };
}

const execute = (runtime, toolName, args, context) =>
  runtime.execute({ toolName, arguments: args ?? {}, context });

async function seedState(runtime, broker, context, { screenshot = false } = {}) {
  const seeded = await execute(
    runtime,
    "get_app_state",
    screenshot ? { app_ref: { pid: 7 }, include_screenshot: true } : { app_ref: { pid: 7 } },
    context,
  );
  assert.equal(seeded.isError, false, `seed get_app_state failed: ${JSON.stringify(seeded)}`);
  return seeded;
}

// ────────────────────────────────────────────── 消费方契约副本（来源逐处标注）

// ① display 投影 = core createCuaToolResultDisplay 的逐行等价缩写
//（apps/mode-cli/packages/core/src/tool/executor/result-display.ts:249-331；
// 字段名/门条件/取值与真身一一对应，32KiB 限长在 canned 输入上恒不触发，未复制 bound）。
const CUA_TARGET_APP_DISPLAY_META_KEY = "mode.cua/target-app-display-v1"; // contracts/tool-result-metadata.ts:23
const MAX_CUA_INLINE_MEDIA_BYTES = 256 * 1024; // result-display.ts:234
const MAX_CUA_INLINE_MEDIA_TOTAL_BYTES = 512 * 1024; // result-display.ts:235

function projectCuaDisplay(result, toolName, { officialCua = true } = {}) {
  const rec = (value) =>
    typeof value === "object" && value !== null && !Array.isArray(value)
      ? value
      : undefined;
  const output = rec(result) ?? {};
  const content = Array.isArray(output.content) ? output.content : [];
  const text = content
    .filter((block) => rec(block)?.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join("\n");
  const structured = rec(output.structuredContent);
  const errorRecord = rec(structured?.error);
  const media = [];
  let inlineBytes = 0;
  for (const item of content) {
    if (
      rec(item)?.type !== "image" ||
      typeof item.mimeType !== "string" ||
      typeof item.data !== "string"
    ) {
      continue;
    }
    const decoded = Buffer.byteLength(item.data, "base64");
    if (
      media.length >= 4 ||
      decoded > MAX_CUA_INLINE_MEDIA_BYTES ||
      inlineBytes + decoded > MAX_CUA_INLINE_MEDIA_TOTAL_BYTES
    ) {
      continue;
    }
    inlineBytes += decoded;
    media.push({ mimeType: item.mimeType, data: item.data });
  }
  const meta = rec(output._meta);
  const targetApp = officialCua
    ? strictParseTargetApp(meta?.[CUA_TARGET_APP_DISPLAY_META_KEY])
    : undefined;
  const permissionStatus =
    officialCua && toolName === "request_access"
      ? cuaRequestAccessStatusSchema.safeParse(meta?.[CUA_REQUEST_ACCESS_STATUS_META_KEY])
      : undefined;
  return {
    kind: "cua",
    schemaVersion: 1,
    toolName,
    status: output.isError === true ? "failed" : "success",
    ...(structured !== undefined ? { structuredContent: JSON.stringify(structured) } : {}),
    ...(text ? { text } : {}),
    ...(typeof errorRecord?.code === "string" ? { errorCode: errorRecord.code } : {}),
    ...(typeof errorRecord?.suggested_action === "string"
      ? { suggestedAction: errorRecord.suggested_action }
      : {}),
    ...(targetApp !== undefined ? { targetApp } : {}),
    ...(permissionStatus?.success ? { permissionStatus: permissionStatus.data } : {}),
    ...(media.length > 0 ? { media } : {}),
  };
}

// ② targetApp strict schema 副本
//（apps/mode-cli/packages/contracts/src/tools/tool-result-metadata.ts:25-46：
// applicationIconLocatorSchema + cuaTargetAppDisplaySchema，.strict() 逐键）。
function strictParseTargetApp(value) {
  const rec = (v) =>
    typeof v === "object" && v !== null && !Array.isArray(v) ? v : undefined;
  const parsed = rec(value);
  if (parsed === undefined) return undefined;
  const ALLOWED = new Set(["schemaVersion", "displayName", "iconLocators"]);
  if (Object.keys(parsed).some((key) => !ALLOWED.has(key))) return undefined; // strict
  if (parsed.schemaVersion !== 1) return undefined;
  const displayName = parsed.displayName;
  if (
    displayName !== undefined &&
    (typeof displayName !== "string" ||
      !displayName.trim() ||
      displayName.trim().length > 512)
  ) {
    return undefined;
  }
  if (!Array.isArray(parsed.iconLocators) || parsed.iconLocators.length > 3) return undefined;
  const MAX_BY_KIND = {
    "darwin-bundle-id": 512,
    "windows-executable-path": 32_768,
    "windows-aumid": 512,
  };
  for (const locator of parsed.iconLocators) {
    const record = rec(locator);
    if (record === undefined) return undefined;
    const keys = Object.keys(record);
    if (keys.length !== 2 || !keys.includes("kind") || !keys.includes("value")) return undefined;
    const max = MAX_BY_KIND[record.kind];
    if (max === undefined) return undefined;
    if (
      typeof record.value !== "string" ||
      !record.value.trim() ||
      record.value.trim().length > max
    ) {
      return undefined;
    }
  }
  return parsed;
}

// ③ strict 形状校验 = shared toolDisplay.ts:67-131 的 kind:"cua" 成员
//（zod .strict() 语义：未知键即拒；此处手演等价断言——跨包黄金见报告移交节）。
function assertStrictCuaDisplay(display, label) {
  const where = label ?? "display";
  assert.equal(display.kind, "cua", `${where}: kind`);
  assert.equal(display.schemaVersion, 1, `${where}: schemaVersion`);
  assert.equal(typeof display.toolName, "string", `${where}: toolName`);
  assert.ok(display.toolName.length > 0, `${where}: toolName 非空`);
  assert.ok(["success", "failed"].includes(display.status), `${where}: status 枚举`);
  const ALLOWED = new Set([
    "kind",
    "schemaVersion",
    "toolName",
    "status",
    "input",
    "structuredContent",
    "text",
    "errorCode",
    "suggestedAction",
    "permissionStatus",
    "targetApp",
    "media",
    "truncated",
  ]);
  for (const key of Object.keys(display)) {
    assert.ok(ALLOWED.has(key), `${where}: strict 未知键 ${key}`);
  }
  for (const key of ["structuredContent", "text", "errorCode", "suggestedAction"]) {
    if (display[key] !== undefined) assert.equal(typeof display[key], "string", `${where}: ${key}`);
  }
  if (display.media !== undefined) {
    assert.ok(Array.isArray(display.media) && display.media.length <= 4, `${where}: media ≤4`);
    for (const item of display.media) {
      assert.equal(typeof item.mimeType, "string", `${where}: media.mimeType`);
      assert.ok(item.mimeType.length > 0, `${where}: media.mimeType 非空`);
      // toolDisplay.ts media.data max(349_528)。
      if (item.data !== undefined) {
        assert.equal(typeof item.data, "string", `${where}: media.data`);
        assert.ok(item.data.length <= 349_528, `${where}: media.data ≤ 349528`);
      }
    }
  }
  if (display.targetApp !== undefined) {
    assert.equal(display.targetApp.schemaVersion, 1, `${where}: targetApp.schemaVersion`);
    assert.ok(Array.isArray(display.targetApp.iconLocators), `${where}: targetApp.iconLocators`);
  }
}

// ④ 帧权威胶水 = core hasOfficialCuaFrameAuthority 的逐行副本
//（apps/mode-cli/packages/core/src/mcp/image-normalization.ts:92-115；其谓词
// OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY / isOfficialCuaImageRefText core 侧
// import 自本包 frame-contract（image-normalization.ts:7-9），此处用同一真身）。
function hasOfficialCuaFrameAuthority(result) {
  if (!result || typeof result !== "object" || Array.isArray(result)) return false;
  if (!Array.isArray(result.content) || !result._meta?.[OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY]) {
    return false;
  }
  return result.content.some((block, index) => {
    if (!block || typeof block !== "object" || block.type !== "image") return false;
    const nextText = result.content[index + 1]?.text;
    return typeof nextText === "string" && isOfficialCuaImageRefText(nextText);
  });
}

// ⑤ media 发现 = UI cuaScreenshotDetails.ts 的 dataUrl 采集
//（字符串分支 :23、记录分支 :41-43、dataUrl 复验 :68——正则逐字复制）。
function findImageDataUrl(value, depth = 0) {
  if (depth > 5) return null;
  if (typeof value === "string") {
    // cuaScreenshotDetails.ts:23
    return /^data:image\/[a-z0-9.+-]+;base64,/iu.test(value) ? value : null;
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findImageDataUrl(item, depth + 1);
      if (found) return found;
    }
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const mimeType =
    typeof value.mimeType === "string"
      ? value.mimeType
      : typeof value.mime_type === "string"
        ? value.mime_type
        : null;
  const data = typeof value.data === "string" ? value.data : null;
  if (mimeType?.startsWith("image/") && data && !data.startsWith("data:")) {
    // cuaScreenshotDetails.ts:41-43
    return `data:${mimeType};base64,${data}`;
  }
  for (const child of Object.values(value)) {
    const found = findImageDataUrl(child, depth + 1);
    if (found) return found;
  }
  return null;
}

// ⑥ 树头解析正则 = UI cuaResultState.ts:28（逐字复制）。
const UI_HEADER_RE = /^app:\s+([A-Za-z0-9.-]+)\s+pid=\d+\s+"([^"\r\n]+)"\s*$/mu;
// ⑦ 元素行正则 = UI cuaResultState.ts:163（逐字复制）。
const uiTargetRowRe = (index) => new RegExp(`^\\s*\\[${index}\\]\\s+(.+)$`, "u");
// ⑧ 列表计数解析 = UI cuaResultState.ts:94-105（逻辑等价副本）。
function parseCuaResultArrayLength(value) {
  if (typeof value !== "string") return null;
  const markerStart = value.lastIndexOf("\n\nStructured content:");
  const candidate = (markerStart >= 0 ? value.slice(0, markerStart) : value).trim();
  if (!candidate) return null;
  try {
    const parsed = JSON.parse(candidate);
    return Array.isArray(parsed) ? parsed.length : null;
  } catch {
    return null;
  }
}

// ────────────────────────────────────────────── 场景（canned runtime 出参，全走真序列化）

async function withRuntime(overlay, fn) {
  const broker = await startBroker(makeHandle(overlay));
  try {
    const runtime = createComputerUseRuntime({ brokerSocketPath: broker.socketPath });
    await fn({ runtime, broker, context: ctx("display") });
  } finally {
    await broker.close();
  }
}

test("帧对观察 → display：kind cua + state_id + media dataUrl + 帧权威副本为真", async () => {
  await withRuntime({ observe: () => observeResult([el(3, "edit", "field", "")]) }, async (h) => {
    const observation = await seedState(h.runtime, h.broker, h.context, { screenshot: true });
    // 帧权威：content 镜像相邻 image@0+ref@1 且 _meta integrity 真值（image-normalization.ts:92）。
    assert.equal(hasOfficialCuaFrameAuthority(observation), true, "帧对观察必须具帧权威");
    assert.equal(isOfficialCuaImageRefText(observation.content[1].text), true);

    const display = projectCuaDisplay(observation, "get_app_state", { officialCua: true });
    assertStrictCuaDisplay(display, "帧对观察 display");
    assert.equal(display.status, "success");
    // structuredContent JSON 含 state_id（UI readCuaResultState 走 display.structuredContent，
    // cuaResultState.ts:193-197）。
    const structured = JSON.parse(display.structuredContent);
    assert.match(structured.state_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u);
    assert.equal(typeof structured.window.window_id, "number");
    assert.ok(Array.isArray(structured.elements));

    // media[0] dataUrl 发现（cuaScreenshotDetails.ts:23/41-43/68 的正则链）。
    assert.equal(display.media.length, 1, "帧对观察恰一个内联媒体");
    const dataUrl = findImageDataUrl(display.media);
    assert.ok(dataUrl, "media 块必须可被 UI dataUrl 发现链采集");
    // cuaScreenshotDetails.ts:68
    const dataUrlMime = /^data:(image\/[a-z0-9.+-]+);base64,/iu.exec(dataUrl);
    assert.ok(dataUrlMime, `dataUrl 形状不符: ${dataUrl?.slice(0, 40)}`);
    assert.equal(dataUrlMime[1], display.media[0].mimeType);
    const bytes = Buffer.byteLength(display.media[0].data, "base64");
    assert.ok(bytes > 0 && bytes <= OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
      `帧 base64 ≤ 200KiB（实际 ${bytes}）`);
    assert.ok(bytes <= MAX_CUA_INLINE_MEDIA_BYTES, "display 内联媒体 256KiB 预算内");
  });
});

test("非帧结果 → 帧权威副本为假（树观察 / 动作收据 / 错误 / request_access）", async () => {
  await withRuntime(
    {
      observe: () => observeResult([el(4, "button", "OK", null)]),
      perform: { ok: false, error: { code: "element_unavailable", message: "element 4 is gone" } },
    },
    async (h) => {
      const treeOnly = await seedState(h.runtime, h.broker, h.context);
      const action = await execute(
        h.runtime,
        "left_click",
        { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
        h.context,
      );
      const access = await execute(h.runtime, "request_access", {}, h.context);
      for (const [label, result] of [
        ["无截图树观察", treeOnly],
        ["失败动作", action],
        ["request_access", access],
      ]) {
        assert.equal(hasOfficialCuaFrameAuthority(result), false, `${label} 不得具帧权威`);
        assert.equal("_meta" in result, false, `${label} 无 _meta`);
      }
      // 失败动作的 errorCode 双落点（index.js errorResult：文本 JSON + structuredContent.error）。
      assert.equal(action.isError, true);
      const failureDisplay = projectCuaDisplay(action, "left_click", { officialCua: true });
      assertStrictCuaDisplay(failureDisplay, "失败动作 display");
      assert.equal(failureDisplay.status, "failed");
      assert.equal(failureDisplay.errorCode, "element_unavailable");
      assert.equal(
        failureDisplay.suggestedAction,
        "Re-observe with get_app_state before acting again.",
      );
      assert.ok(AX_ERROR_CODES.includes(failureDisplay.errorCode), "errorCode ∈ 17 码表");
      assert.equal(JSON.parse(action.content[0].text).code, failureDisplay.errorCode);
      assert.equal(failureDisplay.media, undefined, "错误结果无媒体");
    },
  );
});

test("树文本 → cuaResultState 解析面：header 正则 + 元素行 + display 结构化投影", async () => {
  await withRuntime(
    { observe: () => observeResult([el(5, "button", "OK = null (press)", null)]) },
    async (h) => {
      const observation = await seedState(h.runtime, h.broker, h.context);
      const treeText = observation.content.at(-1).text;
      // cuaResultState.ts:28 的 header 正则逐字匹配首行；分组给出 bundle 段与窗口标题。
      const header = UI_HEADER_RE.exec(treeText);
      assert.ok(header, `header 不匹配: ${JSON.stringify(treeText.split("\n")[0])}`);
      assert.match(header[1], /^[A-Za-z0-9.-]+$/u, "header 名段必须已 sanitize");
      assert.ok(header[2].length > 0, "quoted 标题段非空");
      // cuaResultState.ts:162-164 的消费方式：先按行 split，再对单行跑无 m 标志的行正则。
      const lines = treeText.split(/\r?\n/u);
      const row = lines.map((line) => uiTargetRowRe(5).exec(line)).find(Boolean);
      assert.ok(row, `元素行不匹配: ${treeText}`);
      assert.ok(row[1].includes("button"), row[1]);

      const display = projectCuaDisplay(observation, "get_app_state", { officialCua: true });
      assertStrictCuaDisplay(display, "树观察 display");
      const structured = JSON.parse(display.structuredContent);
      assert.match(structured.state_id, /^[0-9a-f-]{36}$/u);
      assert.equal(structured.snapshot_mode, "full", "首观察全量");
      assert.equal(structured.window.window_id, 99);
      assert.equal(structured.elements[0].index, 5);
      assert.equal(display.media, undefined, "无截图观察不产 media");
      assert.equal(hasOfficialCuaFrameAuthority(observation), false);
    },
  );
});

test("list_apps → 裸数组文本：readCuaResultArrayLength 解析面 + display.text 投影", async () => {
  await withRuntime({}, async (h) => {
    const list = await execute(h.runtime, "list_apps", {}, h.context);
    assert.equal(list.isError, false);
    assert.equal(list.content.length, 1);
    assert.equal("structuredContent" in list, false, "list_apps 无 structuredContent");
    const display = projectCuaDisplay(list, "list_apps", { officialCua: true });
    assertStrictCuaDisplay(display, "list_apps display");
    assert.equal(display.status, "success");
    assert.equal(display.structuredContent, undefined, "无结构化面 → display 不带 structuredContent");
    // cuaResultState.ts:94-105 副本：display.text 解析为裸数组并给出计数。
    const count = parseCuaResultArrayLength(display.text);
    assert.equal(count, GOOD_APPS.length);
    assert.deepEqual(JSON.parse(display.text), GOOD_APPS);
  });
});

test("动作成功 → display 成功面（action_outcome 投影、无 errorCode）", async () => {
  await withRuntime({ observe: () => observeResult([el(4, "button", "OK", null)]) }, async (h) => {
    await seedState(h.runtime, h.broker, h.context);
    const clicked = await execute(
      h.runtime,
      "left_click",
      { target: { type: "element", index: 4 }, app_ref: { pid: 7 } },
      h.context,
    );
    assert.equal(clicked.isError, false, JSON.stringify(clicked));
    const display = projectCuaDisplay(clicked, "left_click", { officialCua: true });
    assertStrictCuaDisplay(display, "动作成功 display");
    assert.equal(display.status, "success");
    assert.equal(display.errorCode, undefined);
    assert.equal(display.suggestedAction, undefined);
    const structured = JSON.parse(display.structuredContent);
    assert.equal(structured.action_sent, true);
    assert.equal(structured.dispatch_status, "delivered");
    assert.equal(structured.action_outcome.action_sent, true);
    assert.equal(display.media, undefined);
  });
});

test("targetApp meta 门：合法携带 → 投影；缺省 / 形状非法 / 非官方 → 省略", async () => {
  await withRuntime({}, async (h) => {
    const list = await execute(h.runtime, "list_apps", {}, h.context);
    const validMeta = {
      schemaVersion: 1,
      displayName: "Notepad",
      iconLocators: [
        { kind: "windows-executable-path", value: "C:\\Windows\\System32\\notepad.exe" },
      ],
    };
    const withMeta = { ...list, _meta: { [CUA_TARGET_APP_DISPLAY_META_KEY]: validMeta } };

    const carried = projectCuaDisplay(withMeta, "list_apps", { officialCua: true });
    assertStrictCuaDisplay(carried, "携带 targetApp display");
    assert.deepEqual(carried.targetApp, validMeta, "合法形状原样投影（strict 过）");

    // 形状非法：iconLocators 超 3 个 / 缺 schemaVersion（contracts:38-46 .strict()）→ 省略。
    const tooMany = {
      schemaVersion: 1,
      iconLocators: [
        { kind: "windows-aumid", value: "a" },
        { kind: "windows-aumid", value: "b" },
        { kind: "windows-aumid", value: "c" },
        { kind: "windows-aumid", value: "d" },
      ],
    };
    for (const bad of [tooMany, { displayName: "x", iconLocators: [] }, { schemaVersion: 1, iconLocators: [], extra: 1 }]) {
      const invalid = projectCuaDisplay(
        { ...list, _meta: { [CUA_TARGET_APP_DISPLAY_META_KEY]: bad } },
        "list_apps",
        { officialCua: true },
      );
      assert.equal(invalid.targetApp, undefined, `非法形状不得投影: ${JSON.stringify(bad)}`);
    }
    // officialCua=false → 即使合法也不投影（result-display.ts:305-307 门）。
    const unofficial = projectCuaDisplay(withMeta, "list_apps", { officialCua: false });
    assert.equal(unofficial.targetApp, undefined);

    // runtime 自身从不携带该 meta（只写帧 integrity + associations，index.js 帧装配节）→ 默认缺省。
    const natural = projectCuaDisplay(list, "list_apps", { officialCua: true });
    assert.equal(natural.targetApp, undefined, "runtime 原生出参无 targetApp");
  });
});

test("request_access → 扁平 AccessStatus 文本 + permissionStatus 恒省略（Windows 无 darwin meta）", async () => {
  await withRuntime({}, async (h) => {
    const access = await execute(h.runtime, "request_access", {}, h.context);
    assert.equal(access.isError, false);
    // spec（docs/specs/computer-use-windows-runtime.md request_access 条）：扁平 AccessStatus。
    const status = JSON.parse(access.content[0].text);
    assert.deepEqual(status, { ready: true, accessibility: "granted", screenRecording: "granted" });
    assert.equal(access.structuredContent.platform, "windows");
    assert.equal("_meta" in access, false, "Windows 不设 darwin-only meta");

    const display = projectCuaDisplay(access, "request_access", { officialCua: true });
    assertStrictCuaDisplay(display, "request_access display");
    assert.equal(display.status, "success");
    assert.equal(display.permissionStatus, undefined, "无 meta → permissionStatus 省略");
    // 即便塞入非法（非 darwin）meta 也省略——真实归因是本包 request-access-contract.js 的
    // 占位契约（safeParse 恒 success:false，非 shared cuaPermission.ts 的 platform:"darwin"
    // 字面 schema 校验拒绝；占位恒失败与"非 darwin 被拒"在 win32 上殊途同归）。
    // meta 键真身取自本包 request-access-contract.js（core result-display.ts:31-35 import 同一模块）。
    const windowsShaped = { schemaVersion: 1, platform: "windows", grantOwner: "user" };
    const spoofed = projectCuaDisplay(
      { ...access, _meta: { [CUA_REQUEST_ACCESS_STATUS_META_KEY]: windowsShaped } },
      "request_access",
      { officialCua: true },
    );
    assert.equal(spoofed.permissionStatus, undefined, "非 darwin 载荷安全失败 → 省略");
    assertStrictCuaDisplay(spoofed, "spoofed request_access display");
  });
});
