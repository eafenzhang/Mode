/* eslint-disable max-lines -- 观察/直通/错误装配/版本预检/位移台账/帧装配同属 runtime 执行面的
   单一状态机（execute 分发与 session 闭包共享 observations/lastFrame 写入路径），拆文件会把同一
   台账的写入点散到多处；本任务提交面限定本文件，行数随 Task 4 动作处理器继续增长。 */
// @mode/cua runtime —— Computer Use host 运行时（Plan B Task 2：骨架 + 直通工具）。
// execute 的三形态结果（规范源 docs/specs/computer-use-windows-runtime.md §接口 runtime 面）：
//   1. 成功：{content, structuredContent?, _meta?, isError:false}
//   2. 失败：{isError:true, content:[单 text 块 = JSON.stringify({code,message,suggested_action?})],
//      structuredContent:{error:{code, suggested_action?}}} —— code 只取 17 键，逐字锁于
//      test/runtime-core.test.mjs；SDK brokerErrorCodeOf 读文本 code、core result-display
//      读 structuredContent.error（双落点缺一不可）。
//   3. 冷启动未就绪：**非 error** 单文本块 {kind:"CUA_NOT_READY", reasonCode:"broker_not_accepting",
//      retryable:true, message} —— 字段与 SDK computer-use-client.mjs notReadyEnvelopeOf 注释
//      钉死的 producer 契约一致（isError===true 会被 SDK 直接丢弃，故信封绝不带 isError）。
// 依赖方向：node: 内置 + 包内互引（broker.js / broker-server.js / frame-contract.js /
// host-display-contract.js / helper/errors.mjs），禁止反向依赖 core/services/ui。
// 零运行时依赖；不落日志（错误即结果）。
import { randomUUID } from "node:crypto";

import { BrokerError, callBrokerMethod, resolveBrokerSocketPath } from "./broker.js";
import { HELPER_PROTOCOL_VERSION } from "./broker-server.js";
import {
  OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY,
  OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
  buildOfficialCuaFrameIntegrityMeta,
  buildOfficialCuaImageRefText,
  readRasterEnvelopeIdentity,
} from "./frame-contract.js";
import {
  CUA_APP_ASSOCIATIONS_MAX_META_BYTES,
  CUA_APP_ASSOCIATIONS_META_KEY,
} from "./host-display-contract.js";
import { AX_ERROR_CODES } from "./helper/errors.mjs";

// SDK COMPUTER_METHOD_NAMES 14 个（computer-use-client.mjs:31-46 逐字）。
// 不在此集合 → method_not_found；在集合内但处理器未注册 → unimplemented
// （get_app_state 已随 Task 3 注册；9 个变更工具由 Task 4 注册后该态消失）。
const TOOL_NAMES = new Set([
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
]);

// SDK MUTATING_METHODS 十个（computer-use-client.mjs:61-72 逐字）。
// stop 之后这些工具被会话闸门拒绝（stop 自身幂等放行）；只读四工具
// （get_app_state / list_apps / list_windows / request_access）保持开放。
const MUTATING_TOOLS = new Set([
  "left_click",
  "scroll",
  "left_click_drag",
  "type",
  "set_value",
  "select_text",
  "key",
  "perform_action",
  "paste",
  "stop_computer_control",
]);

// suggested_action 映射（计划 Task 2 绑定文案，测试逐字锁）；未列出的码省略该字段。
const SUGGESTED_ACTIONS = Object.freeze({
  element_unavailable: "Re-observe with get_app_state before acting again.",
  foreground_required:
    "Target the element index instead, or bring the app to the foreground first.",
  controller_busy:
    "Another computer-control session (or a stopped one) owns control; observe state or ask the user.",
});

// 17 码以 helper/errors.mjs 为唯一来源（与 crates/mode-cua-ax/src/error.rs 同源清单）；
// 码表外一律归 internal —— 绝不把任意文本当码发给 SDK。
const ERROR_CODE_SET = new Set(AX_ERROR_CODES);

// health 预检超时：短于工具调用（30s，= broker.js 默认），冷启动下信封可重试，不必长等。
const HEALTH_TIMEOUT_MS = 5_000;
const BROKER_CALL_TIMEOUT_MS = 30_000;
// 冷启动信封 message（SDK 注释钉死的两句：正在启动 + 稍等重试同一调用）。
const NOT_READY_MESSAGE =
  "The Computer Use helper is starting up. Retry the same tool call after a brief wait.";

// get_app_state 严格 schema：SDK 只会发这四个键（client observe() L593/L607/L612 与
// bindApp L974-982 的实参构造；`.strict()` 语义，凭空发明的键会被 unrecognized_keys 打回）。
const GET_APP_STATE_KEYS = new Set([
  "app_ref",
  "include_screenshot",
  "disable_diffing",
  "tree_shown_to_model",
]);

// 与 Rust observe::DEFAULT_MAX_ELEMENTS 对齐：元素数达此上限时树头附
// `showing A-B of N items`（docs 的容器子项提示口径，上限值随 Rust 常量同步）。
const OBSERVE_MAX_ELEMENTS = 3000;

// 会话键：`${session_id}|${workspace_key ?? workspacePath ?? ""}`（计划原文）。
// d.ts / host parseContext 下发的是 camelCase（sessionId/workspaceKey），brief 写的是
// snake_case —— 两套字段名同一槽位取值，缺 session_id 时只剩 workspace 维分键
// （此时 `|<workspace>`，等价「只按 workspace 分键」）。
function sessionKeyOf(context) {
  const sessionId = context?.session_id ?? context?.sessionId ?? "";
  const workspace =
    context?.workspace_key ?? context?.workspaceKey ?? context?.workspacePath ?? "";
  return `${sessionId}|${workspace}`;
}

// 失败形态唯一装配点：文本 JSON {code, message, suggested_action?} +
// structuredContent.error {code, suggested_action?}（message 不进 structuredContent）。
function errorResult(code, message) {
  const finalCode = ERROR_CODE_SET.has(code) ? code : "internal";
  const text = { code: finalCode, message: String(message) };
  const error = { code: finalCode };
  const suggested = SUGGESTED_ACTIONS[finalCode];
  if (suggested !== undefined) {
    text.suggested_action = suggested;
    error.suggested_action = suggested;
  }
  return {
    isError: true,
    content: [{ type: "text", text: JSON.stringify(text) }],
    structuredContent: { error },
  };
}

// 任意抛出物 → 失败形态（BrokerError.code ∈17 直接用，码外归 internal，message 保留原文）。
function errorResultOf(error) {
  const code = error && typeof error === "object" && typeof error.code === "string"
    ? error.code
    : "internal";
  const message = error instanceof Error ? error.message : String(error);
  return errorResult(code, message);
}

// 冷启动信封：非 error、单文本块、无 structuredContent。
function notReadyResult() {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          kind: "CUA_NOT_READY",
          reasonCode: "broker_not_accepting",
          retryable: true,
          message: NOT_READY_MESSAGE,
        }),
      },
    ],
  };
}

// 连接类失败：callBrokerMethod 把 socket error/close 一律缠成 stale_socket
//（message 保留 ENOENT/ECONNREFUSED 原文）；两路都认，仅在「从未成功 health」时
// 走冷启动信封，热身后断连按普通错误外发（SDK 映射 stale_socket→HELPER_UNAVAILABLE）。
function isConnectionFailure(error) {
  if (error && typeof error === "object" && error.code === "stale_socket") return true;
  const message = error && typeof error === "object" ? String(error.message ?? "") : "";
  return /ECONNREFUSED|ENOENT/u.test(message);
}

// ────────────────────────────────────────────── 直通/合成处理器（Task 2 五工具）

// 单文本块 = 裸 JSON 数组（SDK parseJsonValue 对数组显式分支；无 structuredContent）。
function assertRowArray(payload, toolName) {
  if (Array.isArray(payload)) return payload;
  throw new BrokerError(`${toolName} returned an unexpected payload`, { code: "internal" });
}

async function handleListApps({ call }) {
  const rows = assertRowArray(await call("list_apps"), "list_apps");
  return {
    isError: false,
    content: [{ type: "text", text: JSON.stringify(rows) }],
  };
}

// app_ref → pid 解析（docs：裸字符串读作 bundle_id；{pid} 直连不经 list_apps；
// {name}/{bundle_id} 尽力经 list_apps 解析，不可解析 → invalid_request 点名 app_ref）。
async function resolveAppPid(appRef, call) {
  if (appRef === undefined || appRef === null) {
    throw new BrokerError(
      "list_windows requires app_ref ({pid} | {name} | {bundle_id} | a bundle id string)",
      { code: "invalid_request" },
    );
  }
  const ref = typeof appRef === "string" ? { bundle_id: appRef } : appRef;
  if (typeof ref !== "object" || Array.isArray(ref)) {
    throw new BrokerError("app_ref must be an object or a bundle id string", {
      code: "invalid_request",
    });
  }
  if (Number.isInteger(ref.pid) && ref.pid >= 0) return ref.pid;
  const field =
    typeof ref.bundle_id === "string"
      ? "bundle_id"
      : typeof ref.name === "string"
        ? "name"
        : undefined;
  if (field === undefined) {
    throw new BrokerError("app_ref must carry pid, name or bundle_id", {
      code: "invalid_request",
    });
  }
  const apps = await call("list_apps");
  if (!Array.isArray(apps)) {
    throw new BrokerError("list_apps returned an unexpected payload", { code: "internal" });
  }
  const wanted = ref[field];
  const exact = apps.find((app) => app && app[field] === wanted);
  // name 兜底大小写不敏感（Windows 展示名大小写由 OS 渲染，不作为身份）。
  const loose =
    field === "name"
      ? apps.find((app) => typeof app?.name === "string" && app.name.toLowerCase() === String(wanted).toLowerCase())
      : undefined;
  const match = exact ?? loose;
  if (!match || !Number.isInteger(match.pid)) {
    throw new BrokerError(`no running app matches app_ref ${JSON.stringify(ref)}`, {
      code: "invalid_request",
    });
  }
  return match.pid;
}

async function handleListWindows({ args, call }) {
  const pid = await resolveAppPid(args?.app_ref, call);
  const rows = assertRowArray(await call("list_windows", { pid }), "list_windows");
  return {
    isError: false,
    content: [{ type: "text", text: JSON.stringify(rows) }],
  };
}

// request_access（Windows）：合成，不打 broker。文本 = 扁平 AccessStatus（SDK
// requestAccess 直接 parseJsonRecord 返回它）；structuredContent 承载 UI 投影所需
// 平台位；Windows 无障碍/录屏均非 TCC 管辖 → not_required；不设 darwin-only meta。
function handleRequestAccess() {
  return {
    isError: false,
    content: [
      {
        type: "text",
        text: JSON.stringify({ ready: true, accessibility: "granted", screenRecording: "granted" }),
      },
    ],
    structuredContent: {
      platform: "windows",
      backend: "uia",
      accessibility: { status_after: "not_required" },
      screen_recording: { status_after: "not_required" },
    },
  };
}

// stop_computer_control：置 stopped、释放 lease，幂等（重复调用同形）。
// 收据放 structuredContent 顶层（SDK receiptOf 合并读取；stop ∈ MUTATING 十个 →
// 成功 action_sent:true，未走 Rust dispatch → dispatch_status:"delivered" 表 runtime 已执行）。
function handleStop({ session }) {
  session.stopped = true;
  session.leaseOwner = null;
  return {
    isError: false,
    content: [{ type: "text", text: JSON.stringify({ stopped: true }) }],
    structuredContent: { stopped: true, action_sent: true, dispatch_status: "delivered" },
  };
}

// ────────────────────────────────────────────── 观察面（Task 3：get_app_state）

// 真 addon 出参键名（napi camelCase，实测 dist-cua-helper/cua_ax.node）：
// observe → {windowTitle, focusedIndex, enumerationComplete, elements}；
// list_apps 行 → {pid, name, bundleId, active}；list_windows 行 → {windowId, ...}。
// 历史 fake broker 用 snake_case，两套键名都读（camel 为准）。
function pickField(record, camelKey, snakeKey) {
  return record?.[camelKey] !== undefined ? record[camelKey] : record?.[snakeKey];
}

function firstNonEmptyString(value) {
  return typeof value === "string" && value.trim() ? value : undefined;
}

// 树文本单行化：title/value 内的换行与连续空白压成单空格——元素行必须逐行匹配 UI
// 行正则（/^\s*\[\d+\]\s+(.+)$/u），跨行行会让 UI 的目标行与子行扫描整体错位。
function flattenTreeText(value) {
  return String(value).replace(/\s+/gu, " ").trim();
}

// 头部 <sanitized>：exe/AUMID 显示名 sanitize 到 [A-Za-z0-9.-]+（UI header 正则逐字要求；
// 结构化 app.name 保留原名，两处允许不同）。sanitize 结果为空（如纯 CJK 名）→ app-<pid>。
function sanitizeHeaderName(source, pid) {
  const cleaned = String(source)
    .replace(/[^A-Za-z0-9.-]+/gu, "-")
    .replace(/^[-.]+|[-.]+$/gu, "");
  return cleaned.length > 0 ? cleaned : `app-${pid}`;
}

// observe 元素归一（structuredContent.elements 的八键全集）；形状不符 → 归 internal，
// 绝不把缺 index/kind 的残行喂给渲染或台账。
function normalizeElements(payload) {
  if (!payload || typeof payload !== "object" || !Array.isArray(payload.elements)) {
    throw new BrokerError("observe returned an unexpected payload", { code: "internal" });
  }
  return payload.elements.map((raw) => {
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
      throw new BrokerError("observe returned an unexpected payload", { code: "internal" });
    }
    const index = pickField(raw, "index", "index");
    if (!Number.isInteger(index) || typeof raw.kind !== "string") {
      throw new BrokerError("observe returned an unexpected payload", { code: "internal" });
    }
    return {
      index,
      kind: raw.kind,
      title: typeof raw.title === "string" ? raw.title : null,
      value: typeof raw.value === "string" ? raw.value : null,
      actions: Array.isArray(raw.actions) ? raw.actions.filter((a) => typeof a === "string") : [],
      bounds: Array.isArray(raw.bounds) ? raw.bounds.filter((n) => typeof n === "number") : [],
      enabled: raw.enabled === true,
      offscreen: raw.offscreen === true,
    };
  });
}

// diff 指纹 = kind|title|value|bounds[0..3]（计划原文；null → 空串，bounds 缺位补空）。
function fingerprintOf(element) {
  const [b0, b1, b2, b3] = element.bounds;
  return [
    element.kind,
    element.title ?? "",
    element.value ?? "",
    b0 ?? "",
    b1 ?? "",
    b2 ?? "",
    b3 ?? "",
  ].join("|");
}

// diff：按「index 先配、指纹跨下标补配」计算 added/removed/changed。
// 规则（brief）：index 对不上但指纹在别处出现 → 位移，只计一次 changed（不计 added/removed）。
// 返回 plan（树面行）与 changes（structuredContent 载荷）。
function diffElements(baseline, current) {
  const baseByIndex = new Map(baseline.map((element) => [element.index, element]));
  const curByIndex = new Map(current.map((element) => [element.index, element]));
  const baseByFingerprint = new Map();
  for (const element of baseline) {
    const fingerprint = fingerprintOf(element);
    if (!baseByFingerprint.has(fingerprint)) baseByFingerprint.set(fingerprint, element.index);
  }

  const plan = new Map(); // index → {type:"added"|"changed"|"removed", element}
  const added = [];
  const removed = [];
  const changed = [];
  const consumedBaseline = new Set();

  // 同下标双侧：指纹相同 → 未变更（不入树）；不同 → changed。
  for (const [index, current_] of curByIndex) {
    const base = baseByIndex.get(index);
    if (base === undefined) continue;
    if (fingerprintOf(base) === fingerprintOf(current_)) continue;
    plan.set(index, { type: "changed", element: current_ });
    changed.push(current_);
  }
  // 仅当前下标：基线他处同指纹（且该基线行在当前已消失）→ 位移 changed；否则 added。
  for (const [index, current_] of curByIndex) {
    if (baseByIndex.has(index)) continue;
    const movedFrom = baseByFingerprint.get(fingerprintOf(current_));
    if (movedFrom !== undefined && !curByIndex.has(movedFrom)) {
      plan.set(index, { type: "changed", element: current_ });
      changed.push(current_);
      consumedBaseline.add(movedFrom);
      continue;
    }
    plan.set(index, { type: "added", element: current_ });
    added.push(current_);
  }
  // 仅基线下标：未被位移消费 → removed（位移在上一步已记账，不重复计）。
  for (const [index, base] of baseByIndex) {
    if (curByIndex.has(index) || consumedBaseline.has(index)) continue;
    plan.set(index, { type: "removed", element: base });
    removed.push(base);
  }

  return {
    plan,
    changes: {
      added_count: added.length,
      removed_count: removed.length,
      changed_count: changed.length,
      added,
      removed,
      changed,
    },
  };
}

// 元素行注记（单个 (…) 组）：actions + offscreen 合并——UI readElementName 只剥末尾一个
// (…) 组，拆成两个组会让 ` = ` 右侧变成 "null (press)"，目标名解析整体失真。
function rowTokens(element) {
  return [...element.actions, ...(element.offscreen ? ["offscreen"] : [])];
}

// 元素行：`[<idx>] <kind> <title>[ = <value>][ (<annotation>)]`；` = <value>` 恒渲染
//（缺值渲染字面 null，brief 示例 `[3] button OK = null (press)` 同形）。
function renderElementLine(element, tokens) {
  const rawTitle = typeof element.title === "string" ? element.title.trim() : "";
  const title = rawTitle ? flattenTreeText(rawTitle) : "";
  const body = title ? `${element.kind} ${title}` : element.kind;
  const value =
    element.value === null || element.value === undefined ? "null" : flattenTreeText(element.value);
  const line = `[${element.index}] ${body} = ${value}`;
  return tokens.length > 0 ? `${line} (${tokens.join(", ")})` : line;
}

// 渲染树：首行 header（UI cuaResultState.ts:28 逐字解析）→ 提示行 → 元素行。
// diff 模式只列 added/changed/removed（docs：省略行下标仍有效）。
function renderTree(options) {
  const { pid, headerName, titleText, mode, changes, enumerationComplete, elements, rows } = options;
  const lines = [`app: ${headerName} pid=${pid} "${titleText}"`];
  if (mode === "diff" && changes) {
    lines.push(
      `changes: +${changes.added_count} -${changes.removed_count} ~${changes.changed_count}`,
    );
  }
  if (enumerationComplete === false) lines.push("indices are sparse");
  if (elements.length >= OBSERVE_MAX_ELEMENTS) {
    const total =
      enumerationComplete === false ? `${elements.length}+` : String(elements.length);
    lines.push(`showing 1-${elements.length} of ${total} items`);
  }
  for (const line of rows) lines.push(line);
  return lines.join("\n");
}

// app_ref.window_id 缺省时按 docs 语义重新解析：list_windows(pid) 取「主/键窗口」
// （main||focused，按窗口序首行；每次观察都重新解析，模态切换随之跟走）。
async function resolveObservationWindow(appRef, pid, call) {
  const bound = appRef.window_id;
  if (bound !== undefined) return { windowId: bound, rowTitle: undefined };
  const rows = await call("list_windows", { pid });
  if (!Array.isArray(rows)) {
    throw new BrokerError("list_windows returned an unexpected payload", { code: "internal" });
  }
  const row = rows.find((candidate) => candidate?.main === true || candidate?.focused === true);
  const windowId = row ? pickField(row, "windowId", "window_id") : undefined;
  if (!Number.isInteger(windowId) || windowId < 0) {
    throw new BrokerError(
      `get_app_state could not resolve a main/focused window for app_ref ${JSON.stringify(appRef)} (pid ${pid})`,
      { code: "invalid_request" },
    );
  }
  return { windowId, rowTitle: firstNonEmptyString(row.title) };
}

// 应用身份（structured app / 头部显示名 / associations appKey 共用）：list_apps 按 pid 取行，
// 行缺失回退 app_ref 自带身份（{pid} 裸引用无回退 → 名字退 String(pid)）。
async function resolveAppIdentity(pid, appRef, call) {
  const apps = await call("list_apps");
  const row = Array.isArray(apps)
    ? apps.find((app) => Number.isInteger(app?.pid) && app.pid === pid)
    : undefined;
  return {
    name:
      firstNonEmptyString(pickField(row, "name", "name")) ?? firstNonEmptyString(appRef.name),
    bundleId:
      firstNonEmptyString(pickField(row, "bundleId", "bundle_id")) ??
      firstNonEmptyString(appRef.bundle_id) ??
      firstNonEmptyString(appRef.bundleId),
  };
}

// capture 出参 data 落线形状：JSON 化的 Buffer {type:"Buffer",data:[...]}（真 wire 实测）、
// 裸 base64 字符串、字节数组、Buffer 本体四态都收；取不出 → undefined（fail-closed）。
function captureBase64Of(data) {
  let bytes;
  if (typeof data === "string") return data.length > 0 ? data : undefined;
  if (Buffer.isBuffer(data)) bytes = data;
  else if (Array.isArray(data)) bytes = Buffer.from(data);
  else if (data && typeof data === "object" && data.type === "Buffer" && Array.isArray(data.data)) {
    bytes = Buffer.from(data.data);
  }
  return bytes !== undefined && bytes.length > 0 ? bytes.toString("base64") : undefined;
}

// 截图帧：capture → base64 ≤200KiB 防御（Task 6 质量阶梯落地后此分支应不可达）→
// ref 文本 + integrity meta 一律以真实字节 digest 为准（绝不读 _meta.raster_sha256 校验）。
async function buildFrame(windowId, call) {
  const capture = await call("capture", { windowId });
  if (!capture || typeof capture !== "object") {
    throw new BrokerError("capture returned an unexpected payload", { code: "internal" });
  }
  const base64 = captureBase64Of(capture.data);
  if (base64 === undefined) {
    throw new BrokerError("capture returned no raster bytes", { code: "internal" });
  }
  const length = Buffer.byteLength(base64, "utf8");
  if (length > OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES) {
    // 防御分支：超限帧会被归一化层换成 artifact 文案，模型将失去坐标基准——不发。
    throw new BrokerError(
      `screenshot exceeds the 200KiB inline frame budget (${length} bytes); capture again or observe without include_screenshot`,
      { code: "internal" },
    );
  }
  const width = capture.width;
  const height = capture.height;
  if (!Number.isInteger(width) || width <= 0 || !Number.isInteger(height) || height <= 0) {
    throw new BrokerError("capture returned an invalid raster size", { code: "internal" });
  }
  const mimeType =
    typeof capture.mimeType === "string" && capture.mimeType.startsWith("image/")
      ? capture.mimeType
      : "image/png";
  const imageBlock = { type: "image", data: base64, mimeType };
  const rasterSha256 = readRasterEnvelopeIdentity(imageBlock);
  if (rasterSha256 === undefined) {
    throw new BrokerError("capture returned no raster bytes", { code: "internal" });
  }
  const frameId = randomUUID();
  return {
    imageBlock,
    refText: buildOfficialCuaImageRefText({ frameId, rasterSha256, width, height, mimeType }),
    frameId,
    rasterSha256,
    width,
    height,
    mimeType,
  };
}

// get_app_state 参数二次校验（SDK 已做 unrecognized_keys，spec 要求 runtime 复检）。
function validateAppStateArgs(args) {
  const invalid = (message) => new BrokerError(message, { code: "invalid_request" });
  if (args === null || typeof args !== "object" || Array.isArray(args)) {
    throw invalid("get_app_state requires an arguments object");
  }
  const unknown = Object.keys(args).filter((key) => !GET_APP_STATE_KEYS.has(key));
  if (unknown.length > 0) {
    throw invalid(
      `get_app_state does not accept keys: ${unknown.join(", ")} (strict schema: app_ref, include_screenshot, disable_diffing, tree_shown_to_model)`,
    );
  }
  if (args.app_ref === undefined || args.app_ref === null) {
    throw invalid("get_app_state requires app_ref ({pid} | {name} | {bundle_id} | a bundle id string)");
  }
  for (const key of ["include_screenshot", "disable_diffing", "tree_shown_to_model"]) {
    if (args[key] !== undefined && typeof args[key] !== "boolean") {
      throw invalid(`get_app_state ${key} must be a boolean`);
    }
  }
  let ref =
    typeof args.app_ref === "string"
      ? { bundle_id: args.app_ref } // docs：裸字符串按 bundle id 读
      : args.app_ref;
  if (typeof ref !== "object" || Array.isArray(ref)) {
    throw invalid("app_ref must be an object or a bundle id string");
  }
  const boundWindow = ref.window_id ?? ref.windowId;
  if (boundWindow !== undefined) {
    if (!Number.isInteger(boundWindow) || boundWindow < 0) {
      throw invalid(
        `app_ref.window_id must be a non-negative integer, got ${JSON.stringify(boundWindow)}`,
      );
    }
    ref = { ...ref, window_id: boundWindow }; // 双键名归一到 window_id
  }
  return {
    ref,
    includeScreenshot: args.include_screenshot === true,
    disableDiffing: args.disable_diffing === true,
    shown: args.tree_shown_to_model !== false, // 缺省即「展示给模型」（SDK 默认 true）
  };
}

// 位移台账门（规则 2，Task 4 消费）：只在「下标存在于最新观察」且「最新观察已展示给模型，
// 或其元素序列指纹与最近 shown 基线逐位一致」时放行并返回 bounds；否则 element_unavailable——
// 绝不拿模型没见过的编号去解析（SDK「静默点错元素保护」的 producer 侧落点）。
function resolveElementIndexOf(session, appRefKey, index) {
  const reject = (reason) =>
    new BrokerError(
      `element ${index} is unavailable (${reason}); re-observe with get_app_state before acting again`,
      { code: "element_unavailable" },
    );
  const record = session.observations.get(String(appRefKey));
  if (record === undefined) throw reject("no observation for this window yet");
  if (!Number.isInteger(index)) throw reject("element index must be an integer");
  const element = record.elements.find((candidate) => candidate.index === index);
  if (element === undefined) throw reject("the index is not in the latest observation");
  if (record.shownToModel !== true) {
    const baseline = record.lastShown;
    const identical =
      baseline !== undefined &&
      baseline.fps.length === record.fps.length &&
      record.fps.every((fingerprint, position) => fingerprint === baseline.fps[position]);
    if (!identical) {
      throw reject(
        "the latest observation was hidden and its tree differs from the last shown tree",
      );
    }
  }
  return [...element.bounds];
}

// get_app_state 处理器：严格四参 → pid/窗口/身份解析 → observe →（可选）capture →
// diff/台账落账 → 渲染装配。错误一律抛 BrokerError 走 execute 的 errorResultOf。
async function handleGetAppState({ args, session, call }) {
  const input = validateAppStateArgs(args);
  const pid = await resolveAppPid(input.ref, call);
  const { windowId, rowTitle } = await resolveObservationWindow(input.ref, pid, call);
  const identity = await resolveAppIdentity(pid, input.ref, call);
  const observed = await call("observe", { windowId });
  const elements = normalizeElements(observed);
  // capture 在落账之前：帧失败 → 整体错误结果，本次观察不进台账（模型什么都没看到，
  // 元素表维持上一次成功观察——门的判定与模型所见保持一致）。
  const frame = input.includeScreenshot ? await buildFrame(windowId, call) : undefined;

  const appRefKey = String(windowId);
  const previous = session.observations.get(appRefKey);
  const baseline = previous?.lastShown;
  // 强制全量：disable_diffing / 无基线 / 上一次是纯截图观察（docs：截图后先拿整树）。
  const mode =
    input.disableDiffing || baseline === undefined || previous?.forceFull === true
      ? "full"
      : "diff";
  const diffed = mode === "diff" ? diffElements(baseline.elements, elements) : undefined;

  const shown = input.shown;
  const stateId = randomUUID();
  // 落账（规则 1）：任何观察都更新元素表；只有 shown 观察成为 diff/位移基线；
  // 纯截图观察置位「下一次带树强制全量」，hidden 不动该标志。
  session.observations.set(appRefKey, {
    lastStateId: stateId,
    elements,
    fps: elements.map(fingerprintOf),
    shownToModel: shown,
    lastShown: shown
      ? { stateId, elements, fps: elements.map(fingerprintOf) }
      : baseline,
    forceFull:
      input.includeScreenshot && !shown ? true : shown ? false : previous?.forceFull === true,
  });

  const appName = identity.name ?? String(pid);
  const appKey = identity.bundleId ?? appName ?? String(pid);
  const observedTitle = firstNonEmptyString(pickField(observed, "windowTitle", "window_title"));
  const windowTitle = observedTitle ?? rowTitle ?? "";
  const headerName = sanitizeHeaderName(appName, pid);
  // UI header 的 quoted 段要求至少一个非引号非换行字符：空标题回落显示名，引号转单引号。
  const titleText = (windowTitle ? flattenTreeText(windowTitle).replace(/"/gu, "'") : "") ||
    headerName;

  const content = [];
  if (frame !== undefined) {
    content.push(frame.imageBlock, { type: "text", text: frame.refText });
  }
  if (shown) {
    const rows =
      mode === "diff"
        ? [...diffed.plan.keys()]
            .sort((a, b) => a - b)
            .map((index) => {
              const entry = diffed.plan.get(index);
              return entry.type === "removed"
                ? renderElementLine(entry.element, ["removed"])
                : renderElementLine(entry.element, rowTokens(entry.element));
            })
        : elements.map((element) => renderElementLine(element, rowTokens(element)));
    content.push({
      type: "text",
      text: renderTree({
        pid,
        headerName,
        titleText,
        mode,
        changes: diffed?.changes,
        enumerationComplete: pickField(observed, "enumerationComplete", "enumeration_complete"),
        elements,
        rows,
      }),
    });
  }

  const focusedRaw = pickField(observed, "focusedIndex", "focused_index");
  const structuredContent = {
    state_id: stateId,
    base_state_id: previous?.lastStateId ?? null,
    snapshot_mode: mode,
    app: {
      name: appName,
      pid,
      ...(identity.bundleId !== undefined ? { bundle_id: identity.bundleId } : {}),
    },
    window: { title: windowTitle, window_id: windowId },
    focused_element: Number.isInteger(focusedRaw) ? focusedRaw : null,
    elements,
    ...(diffed !== undefined ? { changes: diffed.changes } : {}),
    ...(frame !== undefined ? { frame_id: frame.frameId } : {}),
  };

  let meta;
  if (frame !== undefined) {
    const associations = { primary: { appKey, displayName: appName } };
    meta = {
      [OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY]: buildOfficialCuaFrameIntegrityMeta({
        frameId: frame.frameId,
        rasterSha256: frame.rasterSha256,
      }),
      // associations 超 16KiB 上限 → 弃用该键（integrity meta 不受牵连）。
      ...(Buffer.byteLength(JSON.stringify(associations), "utf8") <=
      CUA_APP_ASSOCIATIONS_MAX_META_BYTES
        ? { [CUA_APP_ASSOCIATIONS_META_KEY]: associations }
        : {}),
    };
    session.lastFrame = {
      frameId: frame.frameId,
      rasterSha256: frame.rasterSha256,
      width: frame.width,
      height: frame.height,
      mimeType: frame.mimeType,
      windowId,
      appKey,
    };
  }

  return {
    isError: false,
    content,
    structuredContent,
    ...(meta !== undefined ? { _meta: meta } : {}),
  };
}

// 分发表：Task 4 注册 9 个变更工具（action 处理器）—— 同文件追加 `handlers.<toolName> = <fn>`
// 即可，处理器签名 ({args, session, call, signal}) → MCP 结果或抛 BrokerError。
const handlers = Object.create(null);
handlers.list_apps = handleListApps;
handlers.list_windows = handleListWindows;
handlers.request_access = handleRequestAccess;
handlers.stop_computer_control = handleStop;
handlers.get_app_state = handleGetAppState;

export function createComputerUseRuntime(options = {}) {
  const socketPath = options.brokerSocketPath || resolveBrokerSocketPath({ env: options.env });
  const ensureBrokerAvailable = options.ensureBrokerAvailable;
  // refreshMarkerPath 本任务不消费（Task 5 resolver 侧凭据链使用），签名保留。
  const sessions = new Map();
  let disposed = false;
  // health 预检是 runtime 级单次事实（并发 execute 合流到同一 promise）：
  let healthPromise;
  let healthOk = false;
  // 版本失配粘滞：一旦观察到即 runtime 级终身（EVERY subsequent execute →
  // version_mismatch，含合成工具与未知会话）；session.versionSticky 同步标记供
  // Task 3/4 观察（brief 把它列在会话字段里，判定以 runtime 级为准——health 只打一次）。
  let versionMismatch; // {actual, expected} | undefined

  const call = (method, params) =>
    callBrokerMethod({ socketPath, method, params, timeoutMs: BROKER_CALL_TIMEOUT_MS });

  function getSession(context) {
    const key = sessionKeyOf(context);
    let session = sessions.get(key);
    if (session === undefined) {
      session = {
        observations: new Map(), // 每 appRefKey（= 解析出的 window id）一张观察台账（Task 3 填充）
        stopped: false,
        leaseOwner: null,
        lastFrame: null,
        // M4：只写不读的镜像位——判定以 runtime 级 versionMismatch 为准（health 只打一次），
        // 按 brief 保留在会话字段里供 Task 4 观察。
        versionSticky: false,
      };
      // 位移台账门（Task 4 的 action 处理器从 session 直取；appRefKey = String(window_id)）。
      session.resolveElementIndex = (appRefKey, index) =>
        resolveElementIndexOf(session, appRefKey, index);
      sessions.set(key, session);
    }
    return session;
  }

  function runHealthPreflight() {
    return (async () => {
      // 懒启动序列（spec §懒启动与恢复事件序）：首次调用先给 host 一次拉起机会；
      // ensure 失败不直接定罪——是否就绪以 health 实连为准。
      if (typeof ensureBrokerAvailable === "function") {
        try {
          await ensureBrokerAvailable();
        } catch {
          // 拉起失败 → health 照打，连接类失败落 CUA_NOT_READY 信封。
        }
      }
      try {
        const health = await callBrokerMethod({
          socketPath,
          method: "health",
          timeoutMs: HEALTH_TIMEOUT_MS,
        });
        const actual = health?.protocolVersion;
        if (actual === HELPER_PROTOCOL_VERSION) {
          healthOk = true;
          return { kind: "ok" };
        }
        // 版本不符或缺失（helper 旧版不带该字段）→ 粘滞 version_mismatch。
        versionMismatch = {
          actual: typeof actual === "string" && actual.length > 0 ? actual : "(missing)",
          expected: HELPER_PROTOCOL_VERSION,
        };
        return { kind: "version_mismatch" };
      } catch (error) {
        // 未建立成功 health → 清掉合流 promise，下次 execute 重试（冷启动可恢复）。
        healthPromise = undefined;
        if (isConnectionFailure(error)) return { kind: "not_ready" };
        return { kind: "error", error };
      }
    })();
  }

  function preflight() {
    if (versionMismatch !== undefined) return Promise.resolve({ kind: "version_mismatch" });
    if (healthOk) return Promise.resolve({ kind: "ok" });
    healthPromise ??= runHealthPreflight();
    return healthPromise;
  }

  function versionMismatchResult() {
    return errorResult(
      "version_mismatch",
      `helper protocol version mismatch: helper reported ${versionMismatch.actual}, runtime expects ${versionMismatch.expected}`,
    );
  }

  async function execute(input) {
    if (disposed) return errorResult("broker_unavailable", "the runtime has been disposed");
    const { toolName, arguments: args, context, signal } = input ?? {};
    const session = getSession(context);
    // 版本粘滞优先于一切：首次观察到失配的那次与之后每一次都返回 version_mismatch。
    if (versionMismatch !== undefined) {
      session.versionSticky = true;
      return versionMismatchResult();
    }
    // M1（Task 2 评审 carry）：health 预检提到未知工具检查之前——计划字面「runtime 首次
    // execute 前 health」，冷热与版本门先于一切工具级判定（含 method_not_found）。
    const gate = await preflight();
    if (gate.kind === "version_mismatch") {
      session.versionSticky = true;
      return versionMismatchResult();
    }
    if (gate.kind === "not_ready") return notReadyResult();
    if (gate.kind === "error") return errorResultOf(gate.error);
    if (!TOOL_NAMES.has(toolName)) {
      return errorResult("method_not_found", `unknown tool: ${String(toolName)}`);
    }
    // stop 闸门在分发之前给 never-retry 结论（health 预检已按 M1 前移：热身后它是
    // 已决 promise，不产生额外回连）。
    if (
      session.stopped &&
      MUTATING_TOOLS.has(toolName) &&
      toolName !== "stop_computer_control"
    ) {
      return errorResult(
        "controller_busy",
        `computer control was stopped; ${String(toolName)} is refused in this session until a new session takes control`,
      );
    }
    const handler = handlers[toolName];
    if (handler === undefined) {
      return errorResult("unimplemented", `tool ${String(toolName)} is not implemented`);
    }
    try {
      return await handler({ args, session, call, signal });
    } catch (error) {
      return errorResultOf(error);
    }
  }

  async function closeSession(context) {
    sessions.delete(sessionKeyOf(context));
  }

  async function dispose() {
    disposed = true;
    sessions.clear();
    healthPromise = undefined;
  }

  // resolveElementIndex 是内部台账门（不在 index.d.ts 的公开面里；Task 4 处理器经
  // session.resolveElementIndex 取 2 参形态）；runtime 级 3 参形态供测试与跨会话工具直查。
  return {
    execute,
    closeSession,
    dispose,
    resolveElementIndex: (context, appRefKey, index) =>
      resolveElementIndexOf(getSession(context), appRefKey, index),
  };
}
