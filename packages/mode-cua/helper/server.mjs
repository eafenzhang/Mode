// 行缓冲 pipe 服务端：单行 JSON 请求 → Task 1 的 handleRequestLine → 单行 JSON 响应
// （响应不含行尾换行，由本文件补 "\n"——broker.js serializeResponse 的契约）。
import net from "node:net";

import { handleRequestLine, serializeResponse } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import { parseAxError } from "./errors.mjs";

// 请求行上限：正常请求（含 observe/capture 参数）远小于此；无行尾的超长输入既不能成行
// 也无从应答，直接断开，防止无界缓冲撑爆 helper。
const MAX_REQUEST_CHARS = 8 * 1024 * 1024;

// 8 原语映射：broker method（snake_case）与 addon 导出 1:1 直调；参数校验先于 addon 调用，
// addon 抛出的 napi 错误（"<code>:<message>"）统一经 callAddon 重缠成带 code 的 Error，
// 由 dispatchRequest 的 errorResponseFromException 原样带码外发（17 码转译）。
//
// health 是 Helper 进程自身状态（controller 裁决）：真实 addon 没有 health 导出，
// 绝不读 addon —— probeHelperHealth/isExactHealthPid 要求 pid 恒等于 helper 子进程 pid；
// protocolVersion 供 Plan B runtime 连接后首调比对，不符 → 向 SDK 吐 version_mismatch
//（host 的 parseReadyMessage 忽略附加字段，health 是唯一落点）。
const invalidRequest = (message) => Object.assign(new Error(message), { code: "invalid_request" });

// 可选字段的缺席口径：undefined/null 一律按「缺席」——napi Option 显式 null 会被拒
//（Task 3 carry），请求面用「省略键/省略实参」表达缺省。
const isAbsent = (value) => value === undefined || value === null;

function requireU32(value, name) {
  if (isAbsent(value)) throw invalidRequest(`${name} is required`);
  if (!Number.isInteger(value) || value < 0 || value > 0xffff_ffff) {
    throw invalidRequest(`${name} must be a u32`);
  }
  return value;
}

function requireString(value, name) {
  if (isAbsent(value)) throw invalidRequest(`${name} is required`);
  if (typeof value !== "string") throw invalidRequest(`${name} must be a string`);
  return value;
}

// region 元素类型校验（防 napi Vec<i32> 转换抛裸 TypeError → internal）；
// 长度是否四元组归 Rust（capture() 的 invalid_request 文案与码同源，不复制该条件）。
function requireRegion(value) {
  if (!Array.isArray(value) || !value.every((n) => Number.isFinite(n))) {
    throw invalidRequest("region must be an array of finite numbers");
  }
  return value;
}

// addon 调用统一走这里（仅包住 addon 调用；校验错误在外层抛出，不过 parseAxError，
// 否则无前缀的校验文案会被误转成 internal）。napi 抛 "<code>:<msg>" 或裸 TypeError →
// parseAxError 拆 17 码（拆不出归 internal）→ 重缠 {code} 后重抛。
async function callAddon(fn) {
  try {
    return await fn();
  } catch (error) {
    const { code, message } = parseAxError(error);
    throw Object.assign(new Error(message), { code });
  }
}

export function createBackend(addon) {
  return {
    health: () => ({ bundleId: null, pid: process.pid, protocolVersion: HELPER_PROTOCOL_VERSION }),
    // napi list_apps() 无参且同步；dispatchRequest 会 await，Promise 型 launch_app 同样兼容。
    list_apps: () => callAddon(() => addon.list_apps()),
    list_windows: (params) => {
      // pid 缺席 → 省略实参（napi Option 拒绝显式 null）；校验在 callAddon 外，防止被重缠成 internal。
      if (isAbsent(params.pid)) return callAddon(() => addon.list_windows());
      const pid = requireU32(params.pid, "pid");
      return callAddon(() => addon.list_windows(pid));
    },
    observe: (params) => {
      const request = { windowId: requireU32(params.windowId, "windowId") };
      // maxElements 缺席 → 键省略（同上；缺省 3000 在 Rust observe 侧生效）。
      if (!isAbsent(params.maxElements)) {
        request.maxElements = requireU32(params.maxElements, "maxElements");
      }
      return callAddon(() => addon.observe(request));
    },
    capture: (params) => {
      // fullScreen 在 napi 侧是必填 bool（CaptureRequestNapi.full_screen: bool），
      // 请求面缺席 → 合成 false；「fullScreen=false 必须带 windowId」的语义校验归 Rust。
      if (!isAbsent(params.fullScreen) && typeof params.fullScreen !== "boolean") {
        throw invalidRequest("fullScreen must be a boolean");
      }
      const request = { fullScreen: params.fullScreen ?? false };
      if (!isAbsent(params.windowId)) request.windowId = requireU32(params.windowId, "windowId");
      if (!isAbsent(params.region)) request.region = requireRegion(params.region);
      return callAddon(() => addon.capture(request));
    },
    perform: (params) => {
      // payload 是入参面的自由 JSON（形状随 kind 变，Rust parse_req 负责按 kind 校验），
      // 缺键 napi 会拒 → 先拦；给值则原样透传。
      if (params.payload === undefined) throw invalidRequest("payload is required");
      const request = {
        kind: requireString(params.kind, "kind"),
        windowId: requireU32(params.windowId, "windowId"),
        payload: params.payload,
      };
      return callAddon(() => addon.perform(request));
    },
    launch_app: async (params) => {
      // napi 导出是 AsyncTask（Promise，Task 6 裁决 backend 必须 await）；
      // name/bundleId 缺席键省略，类型错 → invalid_request（SDK 保证二选一，双双缺席由 addon 判）。
      const request = {};
      if (!isAbsent(params.name)) request.name = requireString(params.name, "name");
      if (!isAbsent(params.bundleId)) request.bundleId = requireString(params.bundleId, "bundleId");
      return await callAddon(() => addon.launch_app(request));
    },
    screen_probe: () => callAddon(() => addon.screen_probe()),
  };
}

export function createHelperServer(addon) {
  const backend = createBackend(addon);
  return net.createServer((socket) => serveConnection(socket, backend));
}

function serveConnection(socket, backend) {
  socket.setEncoding("utf8");
  let buffered = "";
  // 响应不回显请求 id，顺序即契约：同一连接内按行序串行处理；跨 chunk 分片到达的
  // 数据先拼进 buffered，凑齐 "\n" 才成行。
  let chain = Promise.resolve();
  socket.on("data", (chunk) => {
    buffered += chunk;
    if (buffered.length > MAX_REQUEST_CHARS) {
      socket.destroy();
      return;
    }
    let newlineAt;
    while ((newlineAt = buffered.indexOf("\n")) !== -1) {
      const line = buffered.slice(0, newlineAt);
      buffered = buffered.slice(newlineAt + 1);
      chain = chain.then(() => respond(socket, backend, line)).catch(() => undefined);
    }
  });
  // 客户端提前断开（ECONNRESET/半开连接）不应击穿 helper 进程。
  socket.on("error", () => undefined);
  socket.on("close", () => {
    buffered = "";
  });
}

async function respond(socket, backend, line) {
  const response = await handleRequestLine(backend, line);
  if (!socket.destroyed) socket.write(`${serializeResponse(response)}\n`);
}
