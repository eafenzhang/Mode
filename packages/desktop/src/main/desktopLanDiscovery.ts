import { createSocket } from "node:dgram";
import { execFile } from "node:child_process";
import { networkInterfaces } from "node:os";
import { promisify } from "node:util";
import {
  LAN_ACCESS_DISCOVERY_PORT,
  buildLanProbePayload,
  parseLanAnnouncement,
  type LanDiscoveredPeer,
} from "@mode/shared";

/**
 * 局域网发现（main 侧）：向全局/网段广播地址发一次探测，收集 1.5 秒内的应答。
 * 只发探测、不回周期广播——噪声小，也不会让别人在没被问到时看到我们的存在。
 *
 * Tailscale 补充通道（spec: docs/specs/lan-discovery.md）：tailnet 的 100.x 虚拟网段
 * 不透广播，同窗口内读取本地 `tailscale status --json`，对其中的 IP 逐个单播同一份
 * 探测；只问自己 tailnet 里已知的地址，不做网段/端口扫描。CLI 缺席/超时静默降级。
 */

const DISCOVERY_WINDOW_MS = 1_500;

const execFileAsync = promisify(execFile);

/** Tailscale CLI 候选：PATH 优先，其次 Windows / macOS 的常见安装路径。 */
const TAILSCALE_CLI_CANDIDATES = [
  "tailscale",
  "C:\\Program Files\\Tailscale\\tailscale.exe",
  "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

const TAILSCALE_STATUS_TIMEOUT_MS = 800;

function isIpv4Address(value: string): boolean {
  const parts = value.split(".");
  return (
    parts.length === 4 &&
    parts.every((part) => /^\d{1,3}$/u.test(part) && Number(part) >= 0 && Number(part) <= 255)
  );
}

/**
 * 解析 `tailscale status --json`：收集 Self 与全部 Peer 的 IPv4（去重、滤 IPv6，
 * 离线 peer 也保留——单播过去收不到应答自然过滤）。任何解析失败返回空数组。
 */
export function parseTailscaleStatusIps(stdout: string): string[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null) {
    return [];
  }
  const record = parsed as {
    Self?: { TailscaleIPs?: unknown };
    Peer?: Record<string, { TailscaleIPs?: unknown }>;
  };
  const ips = new Set<string>();
  const collect = (entry?: { TailscaleIPs?: unknown }): void => {
    if (!Array.isArray(entry?.TailscaleIPs)) {
      return;
    }
    for (const ip of entry.TailscaleIPs) {
      if (typeof ip === "string" && isIpv4Address(ip)) {
        ips.add(ip);
      }
    }
  };
  collect(record.Self);
  for (const peer of Object.values(record.Peer ?? {})) {
    collect(peer);
  }
  return [...ips];
}

/** 默认 runner：跑本地 tailscale CLI；未安装/超时/异常一律静默返回空（降级为纯广播）。 */
async function runTailscaleStatusIps(): Promise<string[]> {
  for (const candidate of TAILSCALE_CLI_CANDIDATES) {
    try {
      const { stdout } = await execFileAsync(candidate, ["status", "--json"], {
        timeout: TAILSCALE_STATUS_TIMEOUT_MS,
        windowsHide: true,
      });
      return parseTailscaleStatusIps(stdout);
    } catch {
      // 该候选不可用（未安装 / 超时 / 非零退出）→ 试下一个；全部失败即放弃单播步骤。
    }
  }
  return [];
}

export interface DiscoverLanPeersOptions {
  timeoutMs?: number;
  /** 可注入（测试/诊断）：返回 tailnet 内 IPv4 列表；默认跑本地 tailscale CLI。 */
  tailscaleStatusRunner?: () => Promise<string[]>;
}

function ipv4ToInt(address: string): number | null {
  const parts = address.split(".");
  if (parts.length !== 4) {
    return null;
  }
  let value = 0;
  for (const part of parts) {
    const byte = Number(part);
    if (!Number.isInteger(byte) || byte < 0 || byte > 255) {
      return null;
    }
    value = (value << 8) | byte;
  }
  return value >>> 0;
}

function intToIpv4(value: number): string {
  return [24, 16, 8, 0].map((shift) => (value >>> shift) & 0xff).join(".");
}

/** 由本机地址与掩码推导定向广播地址（部分系统不转发 255.255.255.255）。 */
function computeBroadcastAddress(address: string, netmask: string): string | null {
  const addressInt = ipv4ToInt(address);
  const maskInt = ipv4ToInt(netmask);
  if (addressInt === null || maskInt === null) {
    return null;
  }
  return intToIpv4((addressInt & maskInt) | (~maskInt >>> 0));
}

function collectBroadcastTargets(): string[] {
  const targets = new Set<string>(["255.255.255.255"]);
  for (const infos of Object.values(networkInterfaces())) {
    for (const info of infos ?? []) {
      if (info.family !== "IPv4" || info.internal || !info.netmask) {
        continue;
      }
      const broadcast = computeBroadcastAddress(info.address, info.netmask);
      if (broadcast) {
        targets.add(broadcast);
      }
    }
  }
  return [...targets];
}

export async function discoverLanPeers(
  options: DiscoverLanPeersOptions = {},
): Promise<LanDiscoveredPeer[]> {
  const peers = new Map<string, LanDiscoveredPeer>();
  const socket = createSocket({ type: "udp4", reuseAddr: true });
  const windowMs = options.timeoutMs ?? DISCOVERY_WINDOW_MS;
  const startedAt = Date.now();
  // tailnet 状态读取（≤800ms）与广播探测并行启动，拿到 IP 后在同一窗口内补单播。
  const tailscaleIpsPromise = (options.tailscaleStatusRunner ?? runTailscaleStatusIps)();

  socket.on("message", (message, rinfo) => {
    const announcement = parseLanAnnouncement(message.toString("utf8"));
    if (!announcement) {
      return;
    }
    const key = `${announcement.serverId}@${rinfo.address}:${announcement.port}`;
    peers.set(key, { ...announcement, host: rinfo.address });
  });

  const bound = new Promise<boolean>((resolve) => {
    socket.once("error", () => resolve(false));
    socket.bind(0, "0.0.0.0", () => resolve(true));
  });
  const ok = await bound;
  if (!ok) {
    try {
      socket.close();
    } catch {
      // 绑定失败时 socket 可能已经不可用。
    }
    return [];
  }

  try {
    socket.setBroadcast(true);
    const payload = Buffer.from(buildLanProbePayload(), "utf8");
    for (const target of collectBroadcastTargets()) {
      socket.send(payload, LAN_ACCESS_DISCOVERY_PORT, target, () => {
        // 单个目标发送失败（例如无该网段）不影响其它目标。
      });
    }

    // Tailscale 单播补充：对 tailnet 内已知 IP 逐个发同一份探测（收到应答即入列表，
    // 与广播结果按 serverId@host:port 去重）。读取失败/超时按空处理，只留广播。
    const remainingBeforeProbe = Math.max(0, windowMs - (Date.now() - startedAt));
    let tailscaleIps: string[] = [];
    try {
      tailscaleIps = await Promise.race([
        tailscaleIpsPromise,
        new Promise<string[]>((resolve) =>
          setTimeout(() => resolve([]), Math.max(1, remainingBeforeProbe)),
        ),
      ]);
    } catch {
      tailscaleIps = [];
    }
    for (const ip of tailscaleIps) {
      socket.send(payload, LAN_ACCESS_DISCOVERY_PORT, ip, () => {
        // 单个 tailnet 目标发送失败不影响其它目标（离线设备收不到应答即自然过滤）。
      });
    }

    const remaining = Math.max(0, windowMs - (Date.now() - startedAt));
    await new Promise((resolve) => setTimeout(resolve, remaining));
  } finally {
    try {
      socket.close();
    } catch {
      // 已关闭时忽略。
    }
  }

  return [...peers.values()].sort((left, right) =>
    (left.name ?? left.serverId).localeCompare(right.name ?? right.serverId),
  );
}
