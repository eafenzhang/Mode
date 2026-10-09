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
// 依赖方向：node: 内置 + 包内互引（broker.js / broker-server.js / helper/errors.mjs），
// 禁止反向依赖 core/services/ui。零运行时依赖；不落日志（错误即结果）。
import { BrokerError, callBrokerMethod, resolveBrokerSocketPath } from "./broker.js";
import { HELPER_PROTOCOL_VERSION } from "./broker-server.js";
import { AX_ERROR_CODES } from "./helper/errors.mjs";

// SDK COMPUTER_METHOD_NAMES 14 个（computer-use-client.mjs:31-46 逐字）。
// 不在此集合 → method_not_found；在集合内但处理器未注册 → unimplemented
// （Task 3/4 注册 get_app_state 与 9 个变更工具后该态消失）。
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

// 分发表：Task 3 注册 get_app_state（observe 处理器），Task 4 注册 9 个变更工具
//（action 处理器）—— 同文件追加 `handlers.<toolName> = <fn>` 即可，
// 处理器签名 ({args, session, call, signal}) → MCP 结果或抛 BrokerError。
const handlers = Object.create(null);
handlers.list_apps = handleListApps;
handlers.list_windows = handleListWindows;
handlers.request_access = handleRequestAccess;
handlers.stop_computer_control = handleStop;

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
        observations: new Map(), // 每 appRef 一张观察（Task 3 填充）
        stopped: false,
        leaseOwner: null,
        lastFrame: null,
        versionSticky: false,
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
    if (!TOOL_NAMES.has(toolName)) {
      return errorResult("method_not_found", `unknown tool: ${String(toolName)}`);
    }
    // stop 闸门在预检之前：会话已 stopped 时无需回连 broker 即可给出 never-retry 结论。
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
    const gate = await preflight();
    if (gate.kind === "version_mismatch") {
      session.versionSticky = true;
      return versionMismatchResult();
    }
    if (gate.kind === "not_ready") return notReadyResult();
    if (gate.kind === "error") return errorResultOf(gate.error);
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

  return { execute, closeSession, dispose };
}
