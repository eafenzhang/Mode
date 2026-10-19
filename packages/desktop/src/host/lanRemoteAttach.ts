import { Emitter, SocketProtocol, VSBuffer, type ISocket } from "@mode/rpc";
import { connectViaProtocol } from "@mode/client";
import { hostname } from "node:os";
import {
  buildLanPeerTokenKey,
  buildLanPeerMetaKey,
  serverRemoteInfoSchema,
  MODE_RPC_HOST_CAPABILITY_HEADER,
  type LanAccessPairResult,
  type LanAccessState,
  type ServerRemoteInfo,
} from "@mode/shared";
import type { ICredentialService, IServiceAccessor } from "@mode/services";
import { WebSocket, type RawData } from "ws";

/**
 * 局域网客户端（Host 侧）：配对换令牌、按令牌挂接对端的受信 RPC 通道。
 *
 * 与 SSH/WSL/Docker 不同，这里不部署也不启动任何东西：对端是「已经在跑的 Mode 实例」，
 * 因此跳过 upload/deploy/hello 握手，直接用 HTTP(/api/server-info、/api/rpc-host-capability)
 * + WebSocket(/ws/host) 复用同一套 SocketProtocol → ChannelClient → RemoteServiceAccess。
 */

const LAN_PAIR_TIMEOUT_MS = 10_000;
const LAN_INFO_TIMEOUT_MS = 10_000;

function buildLanHttpBase(host: string, port: number): string {
  return `http://${host}:${port}`;
}

function buildLanWsUrl(host: string, port: number): string {
  return `ws://${host}:${port}/ws/host`;
}

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** 读取对端信息（包含工作区列表）；协议版本不匹配时抛出可读错误。 */
export async function fetchLanPeerInfo(params: {
  host: string;
  port: number;
  token?: string;
}): Promise<ServerRemoteInfo> {
  const url = new URL("/api/server-info", buildLanHttpBase(params.host, params.port));
  if (params.token) {
    url.searchParams.set("token", params.token);
  }
  const response = await fetchWithTimeout(url.toString(), { method: "GET" }, LAN_INFO_TIMEOUT_MS);
  if (response.status === 401) {
    throw new Error("对端拒绝了本次访问：令牌无效或已被重置，需要重新配对");
  }
  if (!response.ok) {
    throw new Error(`读取对端信息失败（HTTP ${response.status}）`);
  }
  const parsed = serverRemoteInfoSchema.safeParse(await response.json());
  if (!parsed.success) {
    throw new Error("对端返回的信息不符合局域网连接协议，请确认两端 Mode 版本一致");
  }
  return parsed.data;
}

/** 用一次性配对码换取该设备专属长期令牌，并写入凭据服务。 */
export async function pairLanPeer(params: {
  credentials: ICredentialService;
  host: string;
  port: number;
  code: string;
  label?: string;
}): Promise<LanAccessPairResult> {
  const response = await fetchWithTimeout(
    new URL("/api/lan/pair", buildLanHttpBase(params.host, params.port)).toString(),
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        code: params.code.trim().toUpperCase(),
        // label = 本机身份（对端「配对我的设备」按它显示设备名）：显式传入优先
        //（验证脚本在用），缺省兜底本机主机名——向导不再传 navigator.platform
        //（Windows 上是 "Win32"，对端列表就会显示 win32）。
        label: params.label?.trim() || hostname(),
      }),
    },
    LAN_PAIR_TIMEOUT_MS,
  );
  if (response.status === 401) {
    throw new Error("配对码无效或已过期，请在对端重新生成");
  }
  if (!response.ok) {
    throw new Error(`配对失败（HTTP ${response.status}）`);
  }
  const payload = (await response.json()) as Partial<LanAccessPairResult>;
  if (typeof payload.token !== "string" || !payload.token) {
    throw new Error("配对响应缺少令牌");
  }
  const result: LanAccessPairResult = {
    token: payload.token,
    serverId: typeof payload.serverId === "string" && payload.serverId ? payload.serverId : "",
    ...(typeof payload.name === "string" && payload.name ? { name: payload.name } : {}),
  };
  if (result.serverId) {
    await params.credentials.save(buildLanPeerTokenKey(result.serverId), result.token);
    // 配对元数据与令牌同生命周期：设置页「我配对的对端」凭它列出地址与名称，
    // 枚举侧只读键名/元数据，不接触令牌本体。
    await params.credentials.save(
      buildLanPeerMetaKey(result.serverId),
      JSON.stringify({
        host: params.host.trim(),
        port: params.port,
        ...(result.name ? { name: result.name } : {}),
        pairedAt: Date.now(),
      }),
    );
  }
  return result;
}

/** 历史重连：按 serverId 取回之前配对得到的令牌。 */
export async function loadLanPeerToken(
  credentials: ICredentialService,
  serverId: string | undefined,
): Promise<string | null> {
  if (!serverId?.trim()) {
    return null;
  }
  return await credentials.load(buildLanPeerTokenKey(serverId.trim()));
}

function wrapNodeWebSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  ws.on("message", (data: RawData, isBinary: boolean) => {
    if (!isBinary) {
      // 该通道只承载二进制 RPC 帧；文本帧一律忽略，避免把协议噪声当帧喂给 ChannelClient。
      return;
    }
    const bytes = Array.isArray(data)
      ? Buffer.concat(data)
      : Buffer.isBuffer(data)
        ? data
        : Buffer.from(data as ArrayBuffer);
    onData.fire(VSBuffer.wrap(new Uint8Array(bytes)));
  });
  ws.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  ws.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(buffer.buffer, { binary: true });
      }
    },
    end() {
      ws.close();
    },
    drain() {
      return Promise.resolve();
    },
    dispose() {
      ws.close();
    },
  };
}

export interface LanRemoteConnection {
  services: IServiceAccessor;
  info: ServerRemoteInfo;
  dispose: () => Promise<void>;
}

/**
 * 挂接对端：申请一次性 host capability（提权票据）→ 以受信中继身份打开 /ws/host。
 * 令牌是长期凭据（配对时下发），capability 只是本次连接的提权声明。
 */
export async function attachLanRemoteConnection(params: {
  host: string;
  port: number;
  token: string;
  signal?: AbortSignal;
  /** 对端通道断开（含进程退出、网络中断）时回调：与 SSH backend.onDidDisconnect 语义一致。 */
  onClose?: () => void;
}): Promise<LanRemoteConnection> {
  const info = await fetchLanPeerInfo({
    host: params.host,
    port: params.port,
    token: params.token,
  });
  if (params.signal?.aborted) {
    throw new Error("远程连接已取消");
  }

  const capabilityResponse = await fetchWithTimeout(
    new URL("/api/rpc-host-capability", buildLanHttpBase(params.host, params.port)).toString(),
    {
      method: "POST",
      // 服务端只从 ?token= 或 mode_lite_token Cookie 取令牌；这里用 Cookie，避免令牌进 URL/日志。
      headers: { cookie: `mode_lite_token=${encodeURIComponent(params.token)}` },
    },
    LAN_INFO_TIMEOUT_MS,
  );
  if (!capabilityResponse.ok) {
    throw new Error(`申请对端连接票据失败（HTTP ${capabilityResponse.status}）`);
  }
  const capabilityPayload = (await capabilityResponse.json()) as { capability?: unknown };
  if (typeof capabilityPayload.capability !== "string" || !capabilityPayload.capability) {
    throw new Error("对端未返回连接票据");
  }

  const ws = new WebSocket(buildLanWsUrl(params.host, params.port), {
    headers: {
      [MODE_RPC_HOST_CAPABILITY_HEADER]: capabilityPayload.capability,
      cookie: `mode_lite_token=${encodeURIComponent(params.token)}`,
    },
  });
  // 修复依据：服务端 ChannelServer 只在建连瞬间发一帧 Initialize，且常与握手回包
  // 同批到达——Node ws 会在同一回调里先 emit open、紧接着处理该帧。若等 open 之后
  // 再挂 message 监听/建协议层，Initialize 会被静默丢弃且不重发，ChannelClient 永久
  // 停在 Uninitialized，所有 RPC 无报错无日志地排队（目录浏览器卡「加载中…」）。
  // 因此 wrap、SocketProtocol 与 ChannelClient 必须在 await open 之前同步构造。
  const services = connectViaProtocol(new SocketProtocol(wrapNodeWebSocket(ws)));
  await new Promise<void>((resolve, reject) => {
    const onOpen = () => {
      ws.off("error", onError);
      resolve();
    };
    const onError = (error: Error) => {
      ws.off("open", onOpen);
      reject(new Error(`连接对端通道失败：${error.message}`));
    };
    ws.once("open", onOpen);
    ws.once("error", onError);
  });

  // 半开连接补偿：ws 关闭/出错都会触发 onClose，让 registry 能立刻把 session 置为断开。
  ws.once("close", () => params.onClose?.());
  ws.once("error", () => params.onClose?.());

  return {
    services,
    info,
    async dispose() {
      try {
        ws.close();
      } catch {
        // 已关闭时忽略。
      }
    },
  };
}

/** 服务端设置卡展示用：把状态转成一行可读地址（无地址时给出提示）。 */
export function formatLanAccessAddresses(state: LanAccessState): string {
  if (!state.enabled || !state.port) {
    return "";
  }
  return state.addresses.length > 0 ? state.addresses.join("、") : `0.0.0.0:${state.port}`;
}
