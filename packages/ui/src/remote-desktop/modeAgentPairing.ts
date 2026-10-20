import type { McpServerConfig, ModeMcpServer } from "@mode/shared";

/**
 * Mode Agent 被控端配对（docs/specs/remote-control-unified-pairing.md）：
 * - 目标机安装器展示配对串；桌面端生成的一次性 LAN 配对码同时服务
 *   `POST /api/remote-agent/enroll` 与 `POST /api/lan/pair`（远程工作区 / 远程桌面）；
 * - 已配对设备 = MCP 配置中以 `mode-agent-` 开头且 `type=http` 的 server；
 * - 令牌只存在 MCP 配置的 Authorization header 中，这里不引入第二份存储。
 */

const MODE_AGENT_MCP_PREFIX = "mode-agent-";

interface ModeAgentMcpMetadata {
  agentId: string;
  name: string;
  host: string;
  port: number;
  pairedAt: number;
  version?: string;
  platform?: string;
}

export interface ModeAgentServerRecord {
  server: ModeMcpServer;
  serverName: string;
  agentId: string;
  name: string;
  host: string;
  port: number;
}

function addressHost(address: string): string {
  const trimmed = address.trim();
  if (trimmed.startsWith("[")) {
    const end = trimmed.indexOf("]");
    return end > 0 ? trimmed.slice(1, end) : trimmed;
  }
  const lastColon = trimmed.lastIndexOf(":");
  return lastColon > 0 ? trimmed.slice(0, lastColon) : trimmed;
}

/** IPv4 地址排序：家用/办公 LAN 优先，Tailscale/CGNAT 其次，链路本地最后。 */
function addressRank(address: string): number {
  const match = addressHost(address).match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/u);
  if (!match) {
    return 6;
  }
  const first = Number(match[1]);
  const second = Number(match[2]);
  if (first === 192 && second === 168) return 0;
  if (first === 10) return 1;
  if (first === 172 && second >= 16 && second <= 31) return 2;
  if (first === 100 && second >= 64 && second <= 127) return 3;
  if (first === 169 && second === 254) return 9;
  return 5;
}

/**
 * 多网卡机器选配对串地址：与 Go 被控端的 primaryPairAddress 同一优先级。
 * 同优先级保持调用方（OS）顺序，避免地址抖动。
 */
export function pickPreferredLanAddress(addresses: readonly string[]): string | undefined {
  let best: { address: string; rank: number } | undefined;
  for (const address of addresses) {
    const rank = addressRank(address);
    if (!best || rank < best.rank) {
      best = { address, rank };
    }
  }
  return best?.address;
}

/**
 * 从 MCP server 列表识别被控端。名称前缀是主判据（设置页编辑后也不会丢），
 * `modeAgent` 元数据只用于展示名/稳定 agentId。
 */
export function readModeAgentServers(servers: readonly ModeMcpServer[]): ModeAgentServerRecord[] {
  const records: ModeAgentServerRecord[] = [];
  for (const server of servers) {
    if (server.source !== "zcodeagentmcp") continue;
    if (!server.name.startsWith(MODE_AGENT_MCP_PREFIX)) continue;
    if (server.config.type !== "http" || typeof server.config.url !== "string") continue;
    let parsed: URL;
    try {
      parsed = new URL(server.config.url);
    } catch {
      continue;
    }
    const metadata = (server.config as McpServerConfig & { modeAgent?: ModeAgentMcpMetadata })
      .modeAgent;
    const host = metadata?.host?.trim() || parsed.hostname;
    const parsedPort = Number(parsed.port || (parsed.protocol === "https:" ? 443 : 80));
    const port = metadata?.port ?? parsedPort;
    records.push({
      server,
      serverName: server.name,
      agentId: metadata?.agentId?.trim() || server.name.slice(MODE_AGENT_MCP_PREFIX.length),
      name: metadata?.name?.trim() || `${host}:${port}`,
      host,
      port,
    });
  }
  return records.sort((left, right) => left.name.localeCompare(right.name));
}

/** 只改展示名，保持 serverName/URL/Authorization 不变（MCP 配置是唯一事实源）。 */
export function renameModeAgentServerConfig(
  config: McpServerConfig,
  record: Pick<ModeAgentServerRecord, "agentId" | "host" | "port">,
  name: string,
): McpServerConfig {
  const nextName = name.trim();
  if (!nextName) {
    return config;
  }
  const metadata = (config as McpServerConfig & { modeAgent?: ModeAgentMcpMetadata }).modeAgent;
  return {
    ...config,
    modeAgent: {
      ...(metadata ?? {}),
      agentId: metadata?.agentId?.trim() || record.agentId,
      name: nextName,
      host: metadata?.host?.trim() || record.host,
      port: metadata?.port ?? record.port,
      pairedAt: metadata?.pairedAt ?? Date.now(),
    },
  } as McpServerConfig;
}

/** 桌面生成码 → 被控端输入的配对串；address 允许是 host:port。 */
export function buildDesktopEnrollmentUri(
  address: string,
  fallbackPort: number | null,
  code: string,
): string {
  const trimmed = address.trim();
  let host = trimmed;
  let port = fallbackPort ?? 0;
  const withPort = trimmed.match(/^\[?([^\]]+?)\]?:(\d{1,5})$/u);
  if (withPort) {
    host = withPort[1] ?? trimmed;
    port = Number(withPort[2]);
  }
  return `mode-agent://enroll?host=${encodeURIComponent(host)}&port=${port}&code=${encodeURIComponent(code)}`;
}

/** 发起对话时预填的指令：明确要求模型使用该被控端的 MCP 工具，而不是本机工具。 */
export function buildModeAgentLaunchPrompt(record: {
  name: string;
  host: string;
  port: number;
  serverName: string;
}): string {
  return [
    `[远程被控端：${record.name}（${record.host}:${record.port}）]`,
    `你正在通过 MCP 服务器 ${record.serverName} 操作这台 Windows 设备。`,
    `请使用 mcp__${record.serverName}__remote_* 工具完成请求；不要用本机 Bash/文件工具代替远程操作。`,
    `桌面操作前先调用 remote_screen_capture，命令优先使用 remote_exec。`,
    "",
    "我的任务是：",
  ].join("\n");
}
