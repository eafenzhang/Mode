// 行缓冲 pipe 服务端：单行 JSON 请求 → Task 1 的 handleRequestLine → 单行 JSON 响应
// （响应不含行尾换行，由本文件补 "\n"——broker.js serializeResponse 的契约）。
import net from "node:net";

import { handleRequestLine, serializeResponse } from "../broker.js";

// 请求行上限：正常请求（含 observe/capture 参数）远小于此；无行尾的超长输入既不能成行
// 也无从应答，直接断开，防止无界缓冲撑爆 helper。
const MAX_REQUEST_CHARS = 8 * 1024 * 1024;

// Task 7 只接 health/list_apps，其余 6 原语由 Task 8 在此补全（YAGNI）。
// health 是 Helper 进程自身状态（controller 裁决）：真实 addon 没有 health 导出，
// 绝不读 addon —— probeHelperHealth/isExactHealthPid 要求 pid 恒等于 helper 子进程 pid。
export function createBackend(addon) {
  return {
    health: () => ({ bundleId: null, pid: process.pid }),
    // napi list_apps() 无参且同步；dispatchRequest 会 await，Task 8 的 Promise 型 launch_app 同样兼容。
    list_apps: () => addon.list_apps(),
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
