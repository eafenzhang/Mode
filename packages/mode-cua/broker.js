import { randomBytes } from "node:crypto";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const BROKER_SOCKET_ENV = "MODE_CUA_PERMISSION_BROKER_SOCKET";
export const BROKER_UNAVAILABLE_ENV = "MODE_CUA_PERMISSION_BROKER_UNAVAILABLE";

export class BrokerError extends Error {
  constructor(message, options = {}) {
    super(message ?? "Computer Use broker is unavailable.");
    this.name = "BrokerError";
    this.code = options.code ?? "unavailable";
    if (options.details !== undefined) this.details = options.details;
  }
}

export class CuaHelperError extends Error {
  constructor(message, options = {}) {
    super(message ?? "Computer Use Helper is unavailable.");
    this.name = "CuaHelperError";
    this.code = options.code ?? "helper_unavailable";
  }
}

export function isCuaHelperError(value) {
  return value instanceof CuaHelperError;
}

const brokerErrorFactory = (code) => (message, details) =>
  new BrokerError(message ?? code, { code, details });

export const notAuthorized = brokerErrorFactory("not_authorized");
export const notSelectable = brokerErrorFactory("not_selectable");
export const notSettable = brokerErrorFactory("not_settable");
export const elementUnavailable = brokerErrorFactory("element_unavailable");
export const actionUnavailable = brokerErrorFactory("action_unavailable");
export const foregroundRequired = brokerErrorFactory("foreground_required");

// socket 路径即凭据：win32 铸造随机命名管道（无口令连接），posix 落临时目录 .sock。
// 16 hex = randomBytes(8)，不可枚举，路径泄露等价于凭据泄露。
export function mintBrokerSocketPath({ dir } = {}) {
  const id = randomBytes(8).toString("hex");
  if (process.platform === "win32" && !dir) return `\\\\.\\pipe\\mode-cua-${id}`;
  return join(dir ?? tmpdir(), `mode-cua-${id}.sock`);
}

// 语义同源：路径即凭据——不另行解析口令或 env 覆盖，需要固定路径的调用方直接持有 socketPath。
export function resolveBrokerSocketPath(options) {
  return mintBrokerSocketPath(options);
}

function serializeRequest(id, method, params) {
  return JSON.stringify({ id, method, params: params ?? {} });
}

export function parseRequestLine(line) {
  let v;
  try {
    v = JSON.parse(line);
  } catch {
    return undefined;
  }
  if (!v || typeof v !== "object" || typeof v.method !== "string") return undefined;
  return { id: typeof v.id === "string" ? v.id : null, method: v.method, params: v.params ?? {} };
}

export function okResponse(result) {
  return { ok: true, result };
}

export function errorResponse(message, { code = "internal" } = {}) {
  return { ok: false, error: { code, message } };
}

export function errorResponseFromException(e) {
  return errorResponse(e instanceof Error ? e.message : String(e), {
    code: (e && typeof e === "object" && typeof e.code === "string" && e.code) || "internal",
  });
}

// 不含行尾换行：由写入方补 "\n"（单请求单响应，一行一条）。
export function serializeResponse(r) {
  return JSON.stringify(r);
}

const BROKER_METHODS = new Set([
  "health",
  "list_apps",
  "list_windows",
  "observe",
  "capture",
  "perform",
  "launch_app",
  "screen_probe",
]);
const READONLY_METHODS = new Set([
  "health",
  "list_apps",
  "list_windows",
  "observe",
  "capture",
  "screen_probe",
]);
export const isBrokerMethod = (m) => BROKER_METHODS.has(m);
export const isReadOnlyBrokerMethod = (m) => READONLY_METHODS.has(m);

export async function dispatchRequest(backend, request) {
  if (!isBrokerMethod(request.method)) {
    return errorResponse(`unknown method: ${request.method}`, { code: "method_not_found" });
  }
  try {
    return okResponse(await backend[request.method](request.params ?? {}));
  } catch (e) {
    return errorResponseFromException(e);
  }
}

export async function handleRequestLine(backend, line) {
  const req = parseRequestLine(line);
  if (!req) return errorResponse("malformed request", { code: "invalid_request" });
  return await dispatchRequest(backend, req);
}

// 客户端：单请求单连接，行分隔 JSON；超时与对端错误一律转 BrokerError。
export function callBrokerMethod({ socketPath, method, params, timeoutMs = 30_000 }) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(socketPath);
    let buf = "";
    let done = false;
    const finish = (fn, v) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      sock.destroy();
      fn(v);
    };
    const timer = setTimeout(
      () => finish(reject, new BrokerError("broker call timed out", { code: "timeout" })),
      timeoutMs,
    );
    sock.on("connect", () => sock.write(serializeRequest("1", method, params) + "\n"));
    sock.on("data", (chunk) => {
      buf += chunk.toString("utf8");
      const nl = buf.indexOf("\n");
      if (nl < 0) return;
      let resp;
      try {
        resp = JSON.parse(buf.slice(0, nl));
      } catch (e) {
        return finish(reject, e);
      }
      if (resp && resp.ok === true) return finish(resolve, resp.result);
      const err = resp && resp.error ? resp.error : {};
      return finish(
        reject,
        new BrokerError(err.message ?? "broker error", { code: err.code ?? "internal" }),
      );
    });
    sock.on("error", (e) => finish(reject, new BrokerError(e.message, { code: "stale_socket" })));
    sock.on("close", () => finish(reject, new BrokerError("connection closed", { code: "stale_socket" })));
  });
}

export async function probeHelperHealth(socketPath, options = {}) {
  const { timeoutMs = 10_000, pollIntervalMs = 100, perTryTimeoutMs = 500 } = options;
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const r = await callBrokerMethod({ socketPath, method: "health", timeoutMs: perTryTimeoutMs });
      if (r && (r.pid === null || typeof r.pid === "number")) {
        return {
          bundleId: typeof r.bundleId === "string" ? r.bundleId : null,
          pid: r.pid ?? null,
        };
      }
      lastError = new Error("health payload invalid");
    } catch (e) {
      lastError = e;
    }
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new BrokerError(`helper health probe failed: ${lastError?.message ?? "unknown"}`, {
    code: "broker_unavailable",
  });
}
