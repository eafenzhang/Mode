import type { McpServerConfig, ModeMcpServer } from "@mode/shared";

/**
 * Mode Agent 被控端配对（docs/specs/legacy-windows-remote-agent.md「一键安装与卸载」）：
 * - 目标机安装器展示配对码/配对 URI；
 * - 桌面端用配对码换长期 token，并把被控端写成一个普通 HTTP MCP server；
 * - 远程控制页以 MCP server 的命名前缀 + `modeAgent` 元数据识别已配对设备。
 *
 * 令牌只存在 MCP 配置的 Authorization header 中；这里不引入第二份存储。
 */

export const MODE_AGENT_MCP_PREFIX = "mode-agent-";
export const MODE_AGENT_MCP_TIMEOUT_MS = 180_000;

export interface ModeAgentPairInput {
  host: string;
  port: number;
  code: string;
  label?: string;
}

export interface ModeAgentPairResult {
  token: string;
  agentId: string;
  name: string;
  hostname?: string;
  version?: string;
  platform?: string;
  port?: number;
  addresses?: string[];
}

export interface ModeAgentMcpMetadata {
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

export interface ParsedModeAgentPairingText {
  host?: string;
  port?: number;
  code: string;
}

/** 去分隔符并大写；安装器展示的 `XXXX-XXXX` 与用户手输的 `xxxxxxxx` 等价。 */
export function normalizeModeAgentCode(value: string): string {
  return value.toUpperCase().replace(/[^0-9A-Z]/gu, "");
}

/**
 * 接受三种输入：
 * - `mode-agent://pair?host=...&port=...&code=...`
 * - `host:port CODE` / `host port code`
 * - 单独的配对码（此时地址/端口由表单字段提供）
 */
export function parseModeAgentPairingText(value: string): ParsedModeAgentPairingText {
  const trimmed = value.trim();
  if (!trimmed) {
    return { code: "" };
  }
  if (/^mode-agent:\/\//iu.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      const rawPort = Number(parsed.searchParams.get("port"));
      return {
        ...(parsed.searchParams.get("host")
          ? { host: parsed.searchParams.get("host") ?? undefined }
          : {}),
        ...(Number.isInteger(rawPort) && rawPort > 0 && rawPort <= 65535
          ? { port: rawPort }
          : {}),
        code: normalizeModeAgentCode(parsed.searchParams.get("code") ?? ""),
      };
    } catch {
      return { code: normalizeModeAgentCode(trimmed) };
    }
  }
  const match = trimmed.match(/^(\S+?)(?::|\s+)(\d{1,5})(?:\s+|[,;]+)([0-9A-Za-z-]{8,12})$/u);
  if (match) {
    const port = Number(match[2]);
    return {
      host: match[1],
      ...(port > 0 && port <= 65535 ? { port } : {}),
      code: normalizeModeAgentCode(match[3] ?? ""),
    };
  }
  return { code: normalizeModeAgentCode(trimmed) };
}

function normalizeHost(rawHost: string): string {
  return rawHost
    .trim()
    .replace(/^https?:\/\//iu, "")
    .replace(/\/.*$/u, "");
}

/** 用配对码换长期 token；返回的 token 由调用方写入 HTTP MCP 配置。 */
export async function pairModeAgent(
  input: ModeAgentPairInput,
  fetchImpl: typeof fetch = fetch,
): Promise<ModeAgentPairResult> {
  let host = normalizeHost(input.host);
  let port = input.port;
  const hostWithPort = host.match(/^(\S+):(\d{1,5})$/u);
  if (hostWithPort) {
    host = hostWithPort[1] ?? host;
    port = Number(hostWithPort[2]);
  }
  if (!host) {
    throw new Error("请填写被控端地址");
  }
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error("端口必须是 1–65535");
  }
  const code = normalizeModeAgentCode(input.code);
  if (code.length !== 8) {
    throw new Error("配对码应为 8 位字符");
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10_000);
  try {
    const response = await fetchImpl(`http://${host}:${port}/v1/pair`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, label: input.label ?? "Mode Desktop" }),
      signal: controller.signal,
    });
    const data = (await response.json().catch(() => null)) as {
      ok?: boolean;
      result?: ModeAgentPairResult;
      error?: { code?: string; message?: string };
    } | null;
    if (!response.ok || !data?.ok || !data.result) {
      throw new Error(data?.error?.message ?? `配对失败（HTTP ${response.status}）`);
    }
    if (!data.result.token?.trim() || !data.result.agentId?.trim()) {
      throw new Error("被控端返回的配对结果不完整");
    }
    return data.result;
  } finally {
    clearTimeout(timer);
  }
}

export function modeAgentServerName(agentId: string): string {
  const safe = agentId
    .toLowerCase()
    .replace(/[^a-z0-9]+/gu, "")
    .slice(0, 16);
  return `${MODE_AGENT_MCP_PREFIX}${safe || "unknown"}`;
}

export function buildModeAgentMcpConfig(
  result: ModeAgentPairResult,
  input: Pick<ModeAgentPairInput, "host" | "port" | "code">,
): McpServerConfig {
  const host = normalizeHost(input.host);
  const metadata: ModeAgentMcpMetadata = {
    agentId: result.agentId,
    name: result.name?.trim() || result.hostname?.trim() || host,
    host,
    port: input.port,
    pairedAt: Date.now(),
    ...(result.version ? { version: result.version } : {}),
    ...(result.platform ? { platform: result.platform } : {}),
  };
  return {
    type: "http",
    url: `http://${host}:${input.port}/mcp`,
    headers: { Authorization: `Bearer ${result.token}` },
    timeoutMs: MODE_AGENT_MCP_TIMEOUT_MS,
    modeAgent: metadata,
  };
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

/** 发起对话时预填的指令：明确要求模型使用该被控端的 MCP 工具，而不是本机工具。 */
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
