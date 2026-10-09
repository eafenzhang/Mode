/* eslint-disable max-lines -- 观察/直通/错误装配/版本预检/位移台账/帧装配同属 runtime 执行面的
   单一状态机（execute 分发与 session 闭包共享 observations/lastFrame 写入路径），拆文件会把同一
   台账的写入点散到多处；本任务提交面限定本文件，行数随 Task 4 动作处理器继续增长。 */
// @mode/cua runtime —— Computer Use host 运行时（Plan B Task 2-4：骨架/直通 → 观察 → 动作面）。
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
//（Task 4 后 14 名全部有处理器：4 直通 + get_app_state + 9 动作工具，该态仅剩防御意义）。
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
// owner 与 key 同源取值但独立成串（workspace 可含 "|"，不能从 key 反拆）：
// `${session_id||"?"}@${workspace||"?"}`，即 spec §controller lease 的抢占键
// (session_id, workspace_key)——缺失槽位记 "?"，让 owner 文案始终可读。
function sessionSlotsOf(context) {
  const sessionId = context?.session_id ?? context?.sessionId ?? "";
  const workspace =
    context?.workspace_key ?? context?.workspaceKey ?? context?.workspacePath ?? "";
  return {
    key: `${sessionId}|${workspace}`,
    owner: `${sessionId || "?"}@${workspace || "?"}`,
  };
}

function sessionKeyOf(context) {
  return sessionSlotsOf(context).key;
}

// 失败形态唯一装配点：文本 JSON {code, message, suggested_action?, details?} +
// structuredContent.error {code, suggested_action?, details?}（message 不进 structuredContent）。
// details（spec §controller lease 的 details.owner）与 suggested_action 同款双落点；
// 注意 SDK assertOk 会用它自造的 details（method/brokerCode）覆盖 producer details
//（computer-use-client.mjs:293-296），模型只看得到 message 文本——owner 之类的关键事实
// 必须同时写进 message，details 只是按 spec 形状留档。
// Task 4：动作面失败若已归并 dispatched 收据（三态之一），收据字段与 error **同层**进
// structuredContent 顶层——SDK receiptOf 读 structuredContent 顶层，unknown →
// possibly_sent 会让 assertOk 抛 actionSent=true 的错（retry=reobserve，绝不盲重放）。
function errorResult(code, message, receipt, details) {
  const finalCode = ERROR_CODE_SET.has(code) ? code : "internal";
  const text = { code: finalCode, message: String(message) };
  const error = { code: finalCode };
  const suggested = SUGGESTED_ACTIONS[finalCode];
  if (suggested !== undefined) {
    text.suggested_action = suggested;
    error.suggested_action = suggested;
  }
  if (details !== undefined && details !== null && typeof details === "object") {
    text.details = details;
    error.details = details;
  }
  return {
    isError: true,
    content: [{ type: "text", text: JSON.stringify(text) }],
    structuredContent: {
      error,
      ...(receipt !== undefined
        ? { action_sent: receipt.action_sent, dispatch_status: receipt.dispatch_status }
        : {}),
    },
  };
}

// 任意抛出物 → 失败形态（BrokerError.code ∈17 直接用，码外归 internal，message 保留原文）。
function errorResultOf(error, receipt) {
  const code = error && typeof error === "object" && typeof error.code === "string"
    ? error.code
    : "internal";
  const message = error instanceof Error ? error.message : String(error);
  return errorResult(code, message, receipt);
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

// 连接类失败：只认 BrokerError code === "stale_socket"——broker.js:169-170 已把 socket
// error/close 事件（含 ECONNREFUSED/ENOENT 冷连失败原文）一律缠成 stale_socket，且
// callBrokerMethod 的其余拒绝路径（超时→timeout、响应错误→对端 code、坏 JSON→internal）
// 都不是连接类，故不再做 message 文本匹配（终审 I3：文本匹配是死分支且会误判同文案的
// 语义错误）。仅在「从未成功 health」时走冷启动信封，热身后断连按普通错误外发
//（SDK 映射 stale_socket→HELPER_UNAVAILABLE），冷/热语义不变。
function isConnectionFailure(error) {
  return Boolean(error && typeof error === "object" && error.code === "stale_socket");
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

// 透明启动轮询界（终审 I1）：launch 后最多 6 次 list_apps、间隔 500ms（6×500ms = 3s 有界，
// 每轮先等后查——CreateProcess 回填与 list_apps 枚举有竞态）；超界仍无行 → launch_failed
//「app did not start」。
const LAUNCH_POLL_ATTEMPTS = 6;
const LAUNCH_POLL_INTERVAL_MS = 500;

// 行键双读（评审 I1）：真 wire 是 napi 驼峰 bundleId（实测 dist-cua-helper），历史 fake
// broker 是 snake —— 与 resolveAppIdentity 的 pickField 保持同一读法，两条解析路径不再分叉。
function appRowValue(field, app) {
  if (app === null || app === undefined) return undefined;
  return field === "bundle_id" ? pickField(app, "bundleId", "bundle_id") : app[field];
}

// 精确命中 + name 兜底大小写不敏感（Windows 展示名大小写由 OS 渲染，不作为身份）。
function findAppRow(apps, field, wanted) {
  const exact = apps.find((app) => appRowValue(field, app) === wanted);
  const loose =
    field === "name"
      ? apps.find(
          (app) =>
            typeof app?.name === "string" &&
            app.name.toLowerCase() === String(wanted).toLowerCase(),
        )
      : undefined;
  return exact ?? loose;
}

// 透明启动（终审 I1；规范源 docs/computer-use.md:448-449 与 SKILL.md:133：getApp 未运行即
// 后台启动，没有独立 launch 工具）：list_apps 未命中且 ref 是 name/bundle_id 形态时
// launch_app（键名按 helper backend 整形：{name} / {bundleId}）→ 有界轮询 list_apps 找新行
// → 回填 pid 继续。{pid} 直连在 resolveAppPid 早退、不进本面（死 pid 无从启动）。
// 失败文案一律带 SDK isAppNotFound 判定短语「target app is not running」
//（computer-use-client.mjs:410-415 的 /target app is not running/，仅字符串 ref 的
// alternateAppRef 换字段重试由此触发）——launch 失败后仍保留该重试面。
async function launchAndResolvePid(field, wanted, ref, call) {
  const launchKey = field === "bundle_id" ? "bundleId" : "name";
  try {
    await call("launch_app", { [launchKey]: wanted });
  } catch (error) {
    // 码照搬 launch_app 的 broker 错误（helper 侧按 17 码表生成，如 launch_failed）；
    // 非字符串码或非 Error 一律归 launch_failed——最终仍经 Task 2 装配器的 17 码闸门。
    const code =
      error && typeof error === "object" && typeof error.code === "string"
        ? error.code
        : "launch_failed";
    throw new BrokerError(
      `target app is not running: launch_app failed for app_ref ${JSON.stringify(ref)} (${error instanceof Error ? error.message : String(error)})`,
      { code },
    );
  }
  for (let attempt = 0; attempt < LAUNCH_POLL_ATTEMPTS; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, LAUNCH_POLL_INTERVAL_MS));
    const rows = await call("list_apps");
    if (!Array.isArray(rows)) continue; // 轮询中的坏帧不终止轮询（终态仍由超界定）
    const launched = findAppRow(rows, field, wanted);
    if (launched !== undefined && Number.isInteger(launched.pid)) return launched.pid;
  }
  throw new BrokerError(
    `target app is not running: launch_app accepted but app_ref ${JSON.stringify(ref)} did not appear in list_apps within ${LAUNCH_POLL_ATTEMPTS * LAUNCH_POLL_INTERVAL_MS}ms (the app did not start)`,
    { code: "launch_failed" },
  );
}

// app_ref → pid 解析（docs：裸字符串读作 bundle_id；{pid} 直连不经 list_apps；
// {name}/{bundle_id} 经 list_apps 解析，未命中 → 透明启动（launchAndResolvePid），
// 其失败一律以 launch 类错误点名 app_ref）。
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
  const match = findAppRow(apps, field, ref[field]);
  if (match !== undefined && Number.isInteger(match.pid)) return match.pid;
  // 未命中 → 透明启动（不再直接抛 not-found：docs 明言绑定即启动、无独立 launch 工具）。
  return await launchAndResolvePid(field, ref[field], ref, call);
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
// 收据放 structuredContent 顶层（SDK receiptOf 合并读取）。
// T2-M5（本任务 carry，只注释不改行为）：**local action, not broker dispatch** —— stop 不经
// Rust perform 的 dispatched 三态，`action_sent:true, dispatch_status:"delivered"` 在此专指
// 「runtime 本地会话动作已执行送达」；与 Task 4 动作收据的 delivered（Rust 证明输入已注入）
// 共用同一键位，语义以本注释区分，避免两种 delivered 混读。
function handleStop({ session }) {
  session.stopped = true;
  // 释放 controller lease（读 session.leaseOwner 判定：非 owner 持有时到不了本 handler——
  // execute 的租约闸门先拒；无人持有时 releaseLease 幂等 no-op）。
  session.releaseLease?.();
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
  // 锁屏预检（终审 I5；spec 风险行 owner = Plan B）：capture 前先 screen_probe——
  // {locked:true} → permission_denied（17 码表无 screen_locked，取语义最近且 never-retry），
  // 零 capture 下发；probe 自身失败按原码外发（诚实报告设施故障，不谎报锁屏）。
  const probe = await call("screen_probe");
  if (probe !== null && typeof probe === "object" && probe.locked === true) {
    throw new BrokerError(
      "the screen is locked; unlock the workstation before capturing a screenshot",
      { code: "permission_denied" },
    );
  }
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
    // 从未 shown 与「shown 过但序列变了」是两种事实，文案分开（评审 M4），同码 fail-closed。
    if (baseline === undefined) {
      throw reject("the latest observation has never been shown to the model");
    }
    const identical =
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
  // Task 4 动作兜底：观察解析出的窗口即会话最近绑定（动作缺 app_ref 时复用同一窗口）。
  session.bound = { appRef: input.ref, windowId };
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
  // appName 恒非空（identity.name ?? String(pid)），末位兜底是死分支（终审 I4 删除）。
  const appKey = identity.bundleId ?? appName;
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

// ────────────────────────────────────────────── 动作面（Task 4：9 个变更工具 + 目标解析 + 收据三态）

// perform dispatched 三态 → 收据（spec §runtime 面；键位与 SDK receiptOf 逐字对齐）：
//   Rust "dispatched"     → {action_sent:true,  dispatch_status:"delivered"}
//   Rust "not_dispatched" → {action_sent:false, dispatch_status:"not_sent"}（addon 证明未下发）
//   Rust "unknown"        → {action_sent:true,  dispatch_status:"possibly_sent"}（无法证明）
const DISPATCH_RECEIPTS = Object.freeze({
  dispatched: Object.freeze({ action_sent: true, dispatch_status: "delivered" }),
  not_dispatched: Object.freeze({ action_sent: false, dispatch_status: "not_sent" }),
  unknown: Object.freeze({ action_sent: true, dispatch_status: "possibly_sent" }),
});

// 复合动作（type：click→type_text）收据合并：action_sent = OR；dispatch_status 取最坏——
// possibly_sent > delivered > not_sent（「可能已下发」必须压过「已下发」，否则模型盲重放）。
const DISPATCH_SEVERITY = Object.freeze({ not_sent: 0, delivered: 1, possibly_sent: 2 });
function mergeDispatchReceipt(left, right) {
  return {
    action_sent: left.action_sent === true || right.action_sent === true,
    dispatch_status:
      DISPATCH_SEVERITY[left.dispatch_status] >= DISPATCH_SEVERITY[right.dispatch_status]
        ? left.dispatch_status
        : right.dispatch_status,
  };
}

// 传输类失败（在途无应答/断连/宿主探测失败）→ 无法证明未下发 → 按 "unknown" 归并收据
//（possibly_sent，SDK 强制 reobserve）。语义类失败（Rust 参数校验/前台门在注入前抛错）
// 不归并：runtime 无从断言「已下发」，保持 Task 2 错误形态（SDK 侧 actionSent 默认 false，
// 与 not_sent 同向；spec 的 not_sent 失败收据在单步语义失败下行为等价，见报告偏差节）。
const TRANSPORT_FAILURE_CODES = new Set(["timeout", "stale_socket", "broker_unavailable"]);

// T4-M2（Task 4 评审 §9.2 carry，Task 7 落地）：set_value 独有的 post-dispatch 语义失败面——
// event 兜底（perform.rs event_set_value：click 聚焦**已注入**后才跑 select_all_and_type）的
// 两道闸门会抛 action_unavailable（input.rs uipi_preflight）/ foreground_required
//（ensure_foreground）；同码也可能来自 click 自身的**注入前**同名闸门，错误响应不带
// dispatched，码级无法二选一。按 spec「无法证明未下发 → possibly_sent」对 set_value 归并
// unknown：多报只多一次 reobserve，漏报会让 SDK actionSent=false 断言「绝未下发」诱发盲重放。
// 其余八工具逐 arm 复核均为注入前失败（task-7 报告 T4-M2 节），维持不带收据的 Task 2 形态。
const SET_VALUE_UNCERTAIN_FAILURE_CODES = new Set([
  "foreground_required",
  "action_unavailable",
]);

const invalidAction = (message) => new BrokerError(message, { code: "invalid_request" });

// 枚举均取 docs/computer-use.md 的工具签名口径（SDK bound API 侧已归一 l/r/m 与 u/d/l/r）。
const MOUSE_BUTTON_SET = new Set(["left", "right", "middle"]);
const SCROLL_DIRECTION_SET = new Set(["up", "down", "left", "right"]);
const RETURN_STATE_SET = new Set(["compact", "full"]);

function actionArgsOf(toolName, args) {
  const value = args === undefined || args === null ? {} : args;
  if (typeof value !== "object" || Array.isArray(value)) {
    throw invalidAction(`${toolName} requires an arguments object`);
  }
  return value;
}

// 二次校验（spec：SDK 已做 unrecognized_keys，runtime 站在翻译位必须给英文诊断）。
function requireStringArg(toolName, args, key, { allowEmpty = true } = {}) {
  const value = args[key];
  if (value === undefined || value === null) {
    throw invalidAction(`${toolName} requires ${key} (string)`);
  }
  if (typeof value !== "string") throw invalidAction(`${toolName} ${key} must be a string`);
  if (!allowEmpty && value.length === 0) {
    throw invalidAction(`${toolName} ${key} must not be empty`);
  }
  return value;
}

function requireTargetArg(toolName, args, key = "target") {
  const target = args[key];
  if (target === undefined || target === null) {
    throw invalidAction(
      `${toolName} requires ${key} ({type:"element",index} or {type:"coordinate",x,y})`,
    );
  }
  return target;
}

// 元素专用工具（select_text / perform_action）：Rust 侧这两种 Req 只有 elementIndex 形态，
// 坐标目标在解析之前就拦下（否则会先撞帧绑定错误，掩盖真实原因）。
function requireElementOnlyTarget(toolName, args, key = "target") {
  const target = requireTargetArg(toolName, args, key);
  const valid =
    target !== null && typeof target === "object" && !Array.isArray(target) && target.type === "element";
  if (!valid) {
    throw invalidAction(
      `${toolName} requires an element target ({type:"element",index}); a coordinate target cannot address an element index`,
    );
  }
  return target;
}

// return_state："compact"|"full"|"none"（docs 签名，缺省 none → 动作后不观察）。
function parseReturnState(toolName, args) {
  const value = args.return_state;
  if (value === undefined || value === null || value === "none") return "none";
  if (RETURN_STATE_SET.has(value)) return value;
  throw invalidAction(
    `${toolName} return_state must be "compact", "full" or "none" (got ${JSON.stringify(value)})`,
  );
}

// 动作的 app/窗口解析（复用 Task 3：resolveAppPid → resolveObservationWindow；台账键规则不变
// appRefKey = String(resolved window_id)，先解析窗口再过位移台账门）。缺 app_ref → 会话最近
// 绑定（SDK bound API 恒发 app_ref，工具层直调可省略）；两者皆无 → invalid_request 点名 app_ref。
async function resolveActionWindow(toolName, appRef, session, call) {
  if (appRef !== undefined && appRef !== null) {
    const ref = typeof appRef === "string" ? { bundle_id: appRef } : appRef; // docs：裸串按 bundle id
    const pid = await resolveAppPid(ref, call);
    const { windowId } = await resolveObservationWindow(ref, pid, call);
    session.bound = { appRef: ref, windowId };
    return { windowId, appRef: ref };
  }
  if (!session.bound) {
    throw invalidAction(
      `${toolName} requires app_ref ({pid} | {name} | {bundle_id} | a bundle id string): no app is bound in this transport`,
    );
  }
  return { windowId: session.bound.windowId, appRef: session.bound.appRef };
}

// coordinate 目标帧绑定（docs + brief 三分支）：给了 frame_id → 与会话 lastFrame 精确比对
//（不匹配/过期被替换/owner 窗口不符 → frame_dispatch_identity_mismatch）；省略 → 绑最近可动作
// 帧（零栅格 → "no actionable frame is available in this transport"）；坐标必须落在帧尺寸内。
function resolveCoordinatePoint(target, session, windowId) {
  const x = target.x;
  const y = target.y;
  if (!Number.isInteger(x) || !Number.isInteger(y)) {
    throw invalidAction(
      `coordinate target must be two integer pixels, got [${JSON.stringify(x)}, ${JSON.stringify(y)}]`,
    );
  }
  const frame = session.lastFrame ?? undefined;
  if (target.frame_id !== undefined && target.frame_id !== null) {
    if (frame === undefined || frame.frameId !== target.frame_id || frame.windowId !== windowId) {
      throw invalidAction(
        `frame_dispatch_identity_mismatch: coordinate frame_id ${JSON.stringify(target.frame_id)} does not name this transport's actionable frame (expired, replaced, or owned by another window)`,
      );
    }
  } else {
    if (frame === undefined) {
      throw invalidAction(
        "no actionable frame is available in this transport; capture one with get_app_state (include_screenshot) before acting on coordinates",
      );
    }
    if (frame.windowId !== windowId) {
      throw invalidAction(
        `frame_dispatch_identity_mismatch: the actionable frame belongs to window ${frame.windowId}, but this action targets window ${windowId}`,
      );
    }
  }
  if (x < 0 || y < 0 || x >= frame.width || y >= frame.height) {
    throw invalidAction(
      `coordinate [${x}, ${y}] lies outside the actionable frame (${frame.width}x${frame.height})`,
    );
  }
  return { x, y };
}

// 目标解析：element → 位移台账门（Task 3 规则 2，session.resolveElementIndex）→ bounds 中心
// floor([x + w/2, y + h/2])，门拒绝（不存在/未 shown/序列漂移）一律 element_unavailable 且
// 零下发；coordinate → 帧绑定。返回 {point, elementIndex?}——elementIndex 是门放行后的原下标
//（Rust observe/perform 同一下标空间，直接进 payload）。
function resolveActionTarget(toolName, target, session, windowId) {
  if (target === null || typeof target !== "object" || Array.isArray(target)) {
    throw invalidAction(
      `${toolName} target must be {type:"element",index} or {type:"coordinate",x,y,frame_id?}`,
    );
  }
  if (target.type === "element") {
    const bounds = session.resolveElementIndex(String(windowId), target.index);
    if (bounds.length < 4) {
      throw new BrokerError(
        `element ${String(target.index)} is unavailable (it exposes no bounds); re-observe with get_app_state before acting again`,
        { code: "element_unavailable" },
      );
    }
    const [bx, by, width, height] = bounds;
    return {
      point: { x: Math.floor(bx + width / 2), y: Math.floor(by + height / 2) },
      elementIndex: target.index,
    };
  }
  if (target.type === "coordinate") {
    return { point: resolveCoordinatePoint(target, session, windowId) };
  }
  throw invalidAction(
    `${toolName} target must be {type:"element",index} or {type:"coordinate",x,y,frame_id?} (got type ${JSON.stringify(target.type)})`,
  );
}

// 门放行后回读元素记录（select_text 缺省整选要观察到的 value 长度）。
function elementValueLengthOf(session, windowId, index) {
  const record = session.observations.get(String(windowId));
  const element = record?.elements.find((candidate) => candidate.index === index);
  return typeof element?.value === "string" ? element.value.length : 0;
}

// broker perform 单步：params {kind, windowId, payload}（helper backend perform 形状）。
// 成功 → dispatched 三态归并；传输类失败 → 先按 unknown 归并再抛；set_value 的不确定码
//（T4-M2，见 SET_VALUE_UNCERTAIN_FAILURE_CODES）同样归并 unknown；dispatched 值不在三态内
//（helper 版本漂移防御）→ 按 unknown 归并 + internal。
async function performStep(toolName, call, windowId, step, noteDispatch) {
  let result;
  try {
    result = await call("perform", { kind: step.kind, windowId, payload: step.payload });
  } catch (error) {
    if (error && typeof error === "object" && TRANSPORT_FAILURE_CODES.has(error.code)) {
      noteDispatch("unknown");
    } else if (
      toolName === "set_value" &&
      error &&
      typeof error === "object" &&
      SET_VALUE_UNCERTAIN_FAILURE_CODES.has(error.code)
    ) {
      noteDispatch("unknown");
    }
    throw error;
  }
  const dispatched = result?.dispatched;
  if (
    dispatched !== "dispatched" &&
    dispatched !== "not_dispatched" &&
    dispatched !== "unknown"
  ) {
    noteDispatch("unknown");
    throw new BrokerError(
      `perform ${step.kind} returned an unexpected dispatched value: ${JSON.stringify(dispatched)}`,
      { code: "internal" },
    );
  }
  noteDispatch(dispatched);
}

// 成功装配：文本块 = 收据载荷；structuredContent = action_outcome（spec 嵌套位）+ 同值顶层键
//（SDK receiptOf 只直读 structuredContent 顶层与 text JSON 的 action_outcome，嵌套在
// structuredContent 里的 action_outcome 它不展开——两处同值，两条读法都能拿到）+
// state_sync_status:"unconfirmed"（动作后 UI 是否变化无从得知；SDK 只读不校验）。
// return_state ≠ none → 复用 Task 3 观察（compact=diff、full=全量，窗口钉死为本次动作窗口），
// 树文本与观察 structuredContent 并入；动作前基线 vs 动作后观察逐指纹 diff 全空 →
// 树尾注 [effect_evidence unchanged]（基线必须在 handleGetAppState 落账之前取出）。
async function assembleActionResult({ session, call, signal, windowId, appRef, receipt, returnState }) {
  const outcome = {
    action_sent: receipt.action_sent,
    dispatch_status: receipt.dispatch_status,
  };
  const structuredContent = {
    action_outcome: { ...outcome },
    ...outcome,
    state_sync_status: "unconfirmed",
  };
  const content = [
    { type: "text", text: JSON.stringify({ ...outcome, state_sync_status: "unconfirmed" }) },
  ];
  if (returnState !== "none") {
    const baseline = session.observations.get(String(windowId))?.lastShown;
    const observation = await handleGetAppState({
      args: {
        app_ref: { ...appRef, window_id: windowId },
        include_screenshot: false,
        disable_diffing: returnState === "full",
        tree_shown_to_model: true,
      },
      session,
      call,
      signal,
    });
    if (baseline !== undefined) {
      const { changes } = diffElements(baseline.elements, observation.structuredContent.elements);
      const unchanged =
        changes.added_count === 0 && changes.removed_count === 0 && changes.changed_count === 0;
      // 动作后观察无截图 → 最后一块即树文本（帧对恒不出现）。
      const treeBlock = observation.content[observation.content.length - 1];
      if (unchanged && treeBlock?.type === "text") {
        treeBlock.text = `${treeBlock.text}\n[effect_evidence unchanged]`;
      }
    }
    content.push(...observation.content);
    Object.assign(structuredContent, observation.structuredContent);
  }
  return { isError: false, content, structuredContent };
}

// 动作执行骨架：同步前置校验（prepare，零 broker 调用）→ 窗口解析 → build steps → 依序
// perform（维护合并收据）→ 成功装配/失败装配。任何阶段失败都带**已归并**收据（若有）走
// Task 2 错误装配——失败时收据是「动作面动没动过」的唯一凭证。
async function runAction(toolName, input, prepare) {
  const { session, call, signal } = input;
  const dispatch = { receipt: undefined };
  const noteDispatch = (state) => {
    const next = DISPATCH_RECEIPTS[state];
    dispatch.receipt =
      dispatch.receipt === undefined ? next : mergeDispatchReceipt(dispatch.receipt, next);
  };
  try {
    const args = actionArgsOf(toolName, input.args);
    const returnState = parseReturnState(toolName, args);
    const build = prepare(args);
    const { windowId, appRef } = await resolveActionWindow(toolName, args.app_ref, session, call);
    const steps = await build({ session, windowId });
    for (const step of steps) {
      await performStep(toolName, call, windowId, step, noteDispatch);
    }
    return await assembleActionResult({
      session,
      call,
      signal,
      windowId,
      appRef,
      receipt: dispatch.receipt,
      returnState,
    });
  } catch (error) {
    return errorResultOf(error, dispatch.receipt);
  }
}

// 分发表：Task 2 五个直通/合成 + Task 3 观察 + Task 4 九个动作处理器——处理器签名
// ({args, session, call, signal}) → MCP 结果或抛 BrokerError；14 名全注册后
// unimplemented 仅剩防御意义（错误装配保留该码，SDK 映射 ACTION_UNAVAILABLE）。
const handlers = Object.create(null);
handlers.list_apps = handleListApps;
handlers.list_windows = handleListWindows;
handlers.request_access = handleRequestAccess;
handlers.stop_computer_control = handleStop;
handlers.get_app_state = handleGetAppState;

// left_click：mouse_button→button、click_count→clickCount、modifiers 直通；target 经门→中心。
// strategy（SDK 会发）一期丢弃：Rust parse_req 的 click 臂无 strategy 键，且 parse_req 只读
// 认识的键、不拒未知键——runtime 构造载荷时不放进任何未知键，事件/无障碍分支由 Rust 内部
// auto 决策（偏差记报告）。
handlers.left_click = (input) =>
  runAction("left_click", input, (args) => {
    const target = requireTargetArg("left_click", args);
    const button =
      args.mouse_button === undefined || args.mouse_button === null ? "left" : args.mouse_button;
    if (!MOUSE_BUTTON_SET.has(button)) {
      throw invalidAction(
        `left_click mouse_button must be left, right or middle (got ${JSON.stringify(args.mouse_button)})`,
      );
    }
    const clickCount =
      args.click_count === undefined || args.click_count === null ? 1 : args.click_count;
    if (!Number.isInteger(clickCount) || clickCount < 1) {
      throw invalidAction(
        `left_click click_count must be a positive integer (got ${JSON.stringify(args.click_count)})`,
      );
    }
    const modifiers =
      args.modifiers === undefined || args.modifiers === null ? "" : args.modifiers;
    if (typeof modifiers !== "string") {
      throw invalidAction("left_click modifiers must be a + separated string");
    }
    return async ({ session, windowId }) => {
      const { point } = resolveActionTarget("left_click", target, session, windowId);
      return [
        { kind: "click", payload: { x: point.x, y: point.y, button, clickCount, modifiers } },
      ];
    };
  });

// left_click_drag：from_target/to 各自解析 → fromX/fromY/toX/toY + modifiers。
handlers.left_click_drag = (input) =>
  runAction("left_click_drag", input, (args) => {
    const fromTarget = requireTargetArg("left_click_drag", args, "from_target");
    const toTarget = requireTargetArg("left_click_drag", args, "to");
    const modifiers =
      args.modifiers === undefined || args.modifiers === null ? "" : args.modifiers;
    if (typeof modifiers !== "string") {
      throw invalidAction("left_click_drag modifiers must be a + separated string");
    }
    return async ({ session, windowId }) => {
      const from = resolveActionTarget("left_click_drag from", fromTarget, session, windowId);
      const to = resolveActionTarget("left_click_drag to", toTarget, session, windowId);
      return [
        {
          kind: "click_drag",
          payload: {
            fromX: from.point.x,
            fromY: from.point.y,
            toX: to.point.x,
            toY: to.point.y,
            modifiers,
          },
        },
      ];
    };
  });

// scroll：scroll_direction→direction、scroll_amount→amount（Rust 侧 clamp 0-100）。
handlers.scroll = (input) =>
  runAction("scroll", input, (args) => {
    const target = requireTargetArg("scroll", args);
    const direction = args.scroll_direction;
    if (!SCROLL_DIRECTION_SET.has(direction)) {
      throw invalidAction(
        `scroll scroll_direction must be up, down, left or right (got ${JSON.stringify(args.scroll_direction)})`,
      );
    }
    const amount = args.scroll_amount;
    if (typeof amount !== "number" || !Number.isFinite(amount)) {
      throw invalidAction(
        `scroll scroll_amount must be a number of pages (got ${JSON.stringify(amount)})`,
      );
    }
    return async ({ session, windowId }) => {
      const { point } = resolveActionTarget("scroll", target, session, windowId);
      return [{ kind: "scroll", payload: { x: point.x, y: point.y, direction, amount } }];
    };
  });

// type：target 给了先 click 聚焦（复合顺序 click→type_text，两次 perform，收据 OR+worst-of），
// 再 type_text {text}。
handlers.type = (input) =>
  runAction("type", input, (args) => {
    const text = requireStringArg("type", args, "text");
    const target = args.target === null ? undefined : args.target;
    return async ({ session, windowId }) => {
      const steps = [];
      if (target !== undefined) {
        const { point } = resolveActionTarget("type", target, session, windowId);
        steps.push({
          kind: "click",
          payload: { x: point.x, y: point.y, button: "left", clickCount: 1, modifiers: "" },
        });
      }
      steps.push({ kind: "type_text", payload: { text } });
      return steps;
    };
  });

// set_value：element → {elementIndex, value}；coordinate → {x, y, value}（Rust 聚焦点形态）。
handlers.set_value = (input) =>
  runAction("set_value", input, (args) => {
    const target = requireTargetArg("set_value", args);
    const value = requireStringArg("set_value", args, "value");
    return async ({ session, windowId }) => {
      const resolved = resolveActionTarget("set_value", target, session, windowId);
      const payload =
        resolved.elementIndex !== undefined
          ? { elementIndex: resolved.elementIndex, value }
          : { x: resolved.point.x, y: resolved.point.y, value };
      return [{ kind: "set_value", payload }];
    };
  });

// select_text：element 专用；text_range [start, length] 解构成 start/length。缺省（docs：
// 整选）→ start 0 + 观察值全长——Rust select_text 首行拒负 length（perform.rs
// start<0||length<0 → invalid_request），{start:0,length:-1} 不可用，故按观察 value 取长；
// value 为 null → 0（Rust 接受 length 0，无值即无从选起）。
handlers.select_text = (input) =>
  runAction("select_text", input, (args) => {
    const target = requireElementOnlyTarget("select_text", args);
    const range = args.text_range === null ? undefined : args.text_range;
    let explicit;
    if (range !== undefined) {
      if (!Array.isArray(range) || range.length !== 2) {
        throw invalidAction(
          `select_text text_range must be [start, length] (got ${JSON.stringify(range)})`,
        );
      }
      const [start, length] = range;
      if (!Number.isInteger(start) || !Number.isInteger(length) || start < 0 || length < 0) {
        throw invalidAction(
          `select_text text_range start/length must be non-negative integers (got ${JSON.stringify(range)})`,
        );
      }
      explicit = { start, length };
    }
    return async ({ session, windowId }) => {
      const { elementIndex } = resolveActionTarget("select_text", target, session, windowId);
      const start = explicit?.start ?? 0;
      const length = explicit?.length ?? elementValueLengthOf(session, windowId, elementIndex);
      return [{ kind: "select_text", payload: { elementIndex, start, length } }];
    };
  });

// key：text→chord（SDK 已做键位归一）、repeat 直通、hold_seconds→holdMs（×1000，Rust clamp
// 10s）；strategy 一期丢弃（同 left_click 注释）。
handlers.key = (input) =>
  runAction("key", input, (args) => {
    const chord = requireStringArg("key", args, "text", { allowEmpty: false });
    const repeat = args.repeat === undefined || args.repeat === null ? 1 : args.repeat;
    if (!Number.isInteger(repeat) || repeat < 0) {
      throw invalidAction(
        `key repeat must be a non-negative integer (got ${JSON.stringify(args.repeat)})`,
      );
    }
    const holdSeconds =
      args.hold_seconds === undefined || args.hold_seconds === null ? 0 : args.hold_seconds;
    if (typeof holdSeconds !== "number" || !Number.isFinite(holdSeconds) || holdSeconds < 0) {
      throw invalidAction(
        `key hold_seconds must be a non-negative number (got ${JSON.stringify(args.hold_seconds)})`,
      );
    }
    return async () => [
      { kind: "key", payload: { chord, repeat, holdMs: Math.round(holdSeconds * 1000) } },
    ];
  });

// paste：载荷只有 {text}——format（SDK 工具层会发）一期丢弃：Rust Req::Paste 只读 text，
// 剪贴板一律按纯文本写入（偏差记报告）。
handlers.paste = (input) =>
  runAction("paste", input, (args) => {
    const text = requireStringArg("paste", args, "text");
    return async () => [{ kind: "paste", payload: { text } }];
  });

// perform_action：element 专用（Rust Req::Action 只有 elementIndex）。
handlers.perform_action = (input) =>
  runAction("perform_action", input, (args) => {
    const target = requireElementOnlyTarget("perform_action", args);
    const action = requireStringArg("perform_action", args, "action", { allowEmpty: false });
    return async ({ session, windowId }) => {
      const { elementIndex } = resolveActionTarget("perform_action", target, session, windowId);
      return [{ kind: "action", payload: { elementIndex, action } }];
    };
  });

export function createComputerUseRuntime(options = {}) {
  const socketPath = options.brokerSocketPath || resolveBrokerSocketPath({ env: options.env });
  const ensureBrokerAvailable = options.ensureBrokerAvailable;
  // refreshMarkerPath 本任务不消费（Task 5 resolver 侧凭据链使用），签名保留。
  const sessions = new Map();
  let disposed = false;
  // controller lease（spec §controller lease + 所有权表；终审 I2 最小实现）：单活跃控制者，
  // 抢占键 = (session_id, workspace_key)（= sessionSlotsOf().owner）。runtime 级记录是唯一
  // 判定事实；持有会话的 session.leaseOwner 指向**同一条记录**（读路径见 releaseLease 与
  // 执行闸门——不再是只写死字段）。释放点：owner 的 stop / closeSession、dispose。
  let lease = null; // {owner, at, sessionKey} | null
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
    const key = sessionSlotsOf(context).key;
    let session = sessions.get(key);
    if (session === undefined) {
      session = {
        observations: new Map(), // 每 appRefKey（= 解析出的 window id）一张观察台账（Task 3 填充）
        stopped: false,
        leaseOwner: null,
        lastFrame: null,
        // Task 4：会话最近一次解析出的动作作用域 {appRef(归一对象), windowId}——动作缺
        // app_ref 时的兜底（docs：app_ref 可省；SDK bound API 恒发，工具层直调可能省略）。
        bound: null,
        // M4：只写不读的镜像位——判定以 runtime 级 versionMismatch 为准（health 只打一次），
        // 按 brief 保留在会话字段里供 Task 4 观察。
        versionSticky: false,
      };
      // 位移台账门（Task 4 的 action 处理器从 session 直取；appRefKey = String(window_id)）。
      session.resolveElementIndex = (appRefKey, index) =>
        resolveElementIndexOf(session, appRefKey, index);
      // 释放本会话持有的 controller lease（stop / closeSession 共用）：读 session.leaseOwner
      // 判定「本会话确实持有」（持有记录与 runtime lease 恒为同一对象），非持有者不动别人的租约。
      session.releaseLease = () => {
        const record = session.leaseOwner;
        if (record === null) return;
        if (lease === record) lease = null;
        session.leaseOwner = null;
      };
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
    // controller lease 闸门（终审 I2，spec §controller lease）：抢占发生在**通过 stopped 闸门后
    // 的首个变更调用**上（stop 只释放不抢占，但同样受本闸门约束）。
    //   - 无人持有 → 记 {owner, at}（同 session 重入 = 已是 owner，放行）；
    //   - 他人他 workspace 持有 → controller_busy，**owner 必须写进 message**：SDK assertOk
    //     用它自造的 details 覆盖 producer details（computer-use-client.mjs:293-296），
    //     模型只看得到 message 文本；details.owner 仍按 spec 形状落双落点（errorResult）。
    //   - 只读四工具（get_app_state/list_apps/list_windows/request_access）不进本闸门。
    if (MUTATING_TOOLS.has(toolName)) {
      const slots = sessionSlotsOf(context);
      if (lease !== null && lease.owner !== slots.owner) {
        return errorResult(
          "controller_busy",
          `computer control is held by another session (owner ${lease.owner}); ${String(toolName)} is refused until that session stops or closes`,
          undefined,
          { owner: lease.owner },
        );
      }
      if (lease === null && toolName !== "stop_computer_control") {
        const record = { owner: slots.owner, at: Date.now(), sessionKey: slots.key };
        lease = record;
        session.leaseOwner = record;
      }
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
    const key = sessionKeyOf(context);
    // owner closeSession 释放租约（releaseLease 内判 owner；他人会话只删自己的会话）。
    sessions.get(key)?.releaseLease?.();
    sessions.delete(key);
  }

  async function dispose() {
    disposed = true;
    sessions.clear();
    lease = null;
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
