/**
 * 局域网连接端到端校验（手动 / CI 可选）：
 *   1) 未授权访问 /api/server-info 必须 401
 *   2) 用对端显示的配对码 POST /api/lan/pair 换长期令牌
 *   3) 带令牌 GET /api/server-info 拿到工作区列表
 *   4) POST /api/rpc-host-capability 申请一次性提权票据
 *   5) 带票据 + 令牌升级 /ws/host（受信中继通道）
 *   6) 通过通道跑一次真实 RPC（IFileService.readdir）
 *
 * 用法：node --import tsx packages/desktop/scripts/verify-lan-attach.mjs <host> <port> <pairCode> [dir]
 */
import { Emitter, SocketProtocol, VSBuffer } from "@mode/rpc";
import { connectViaProtocol } from "@mode/client";
import { serverRemoteInfoSchema, MODE_RPC_HOST_CAPABILITY_HEADER } from "@mode/shared";
import { WebSocket } from "ws";

const [host, portRaw, code, dir = process.cwd()] = process.argv.slice(2);
const port = Number(portRaw);
if (!host || !Number.isFinite(port) || !code) {
  console.error("usage: node --import tsx packages/desktop/scripts/verify-lan-attach.mjs <host> <port> <pairCode> [dir]");
  process.exit(2);
}
const base = `http://${host}:${port}`;

const unauth = await fetch(`${base}/api/server-info`);
console.log("1) 未授权 server-info:", unauth.status);
if (unauth.status !== 401) {
  console.error("   ✗ 期望 401（局域网服务必须拒绝未配对访问）");
  process.exit(1);
}

const pairResponse = await fetch(`${base}/api/lan/pair`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ code, label: "verify-lan-attach" }),
});
if (!pairResponse.ok) {
  console.error("2) 配对失败:", pairResponse.status, await pairResponse.text());
  process.exit(1);
}
const pair = await pairResponse.json();
console.log("2) 配对成功: serverId=", pair.serverId, " token.length=", pair.token.length);

const infoResponse = await fetch(`${base}/api/server-info?token=${encodeURIComponent(pair.token)}`);
const info = serverRemoteInfoSchema.parse(await infoResponse.json());
console.log("3) server-info:", info.name, "workspaces:", info.workspaces.length);
for (const workspace of info.workspaces.slice(0, 5)) {
  console.log("   -", workspace.label, workspace.path);
}

const capabilityResponse = await fetch(`${base}/api/rpc-host-capability`, {
  method: "POST",
  headers: { cookie: `zcode_lite_token=${encodeURIComponent(pair.token)}` },
});
const capability = await capabilityResponse.json();
console.log("4) capability ticket:", capabilityResponse.status, Boolean(capability.capability));
if (!capability.capability) {
  process.exit(1);
}

const ws = new WebSocket(`ws://${host}:${port}/ws/host`, {
  headers: {
    [MODE_RPC_HOST_CAPABILITY_HEADER]: capability.capability,
    cookie: `zcode_lite_token=${encodeURIComponent(pair.token)}`,
  },
});
await new Promise((resolve, reject) => {
  ws.once("open", () => resolve());
  ws.once("error", reject);
});
console.log("5) /ws/host 已升级为受信通道");

const onData = new Emitter();
const socket = {
  onData: onData.event,
  onClose: new Emitter().event,
  onEnd: new Emitter().event,
  write(buffer) {
    ws.send(buffer.buffer, { binary: true });
  },
  end: () => ws.close(),
  drain: () => Promise.resolve(),
  dispose: () => ws.close(),
};
ws.on("message", (data, isBinary) => {
  if (!isBinary) {
    return;
  }
  const bytes = Array.isArray(data) ? Buffer.concat(data) : Buffer.from(data);
  onData.fire(VSBuffer.wrap(new Uint8Array(bytes)));
});

const services = connectViaProtocol(new SocketProtocol(socket));
const fileService = services.fileService;
const entries = await fileService.readdir({ path: dir });
console.log(`6) RPC 往返成功 — readdir(${dir}) 返回 ${entries.length} 项`);
ws.close();
console.log("ALL GOOD");
