import type { LanPeerEndpoint } from "./lanAccess.js";
import type { RemoteAssetInstallMode } from "./remoteAssetInstallMode.js";
import type { RemoteResourcePackageSelection } from "./remoteResourcePackages.js";

export interface SSHConnectOptions {
  kind: "ssh";
  host: string;
  port?: number;
  username: string;
  sshConfigAlias?: string;
  password?: string;
  privateKeyPath?: string;
  privateKeyPassphrase?: string;
  assetInstallMode?: RemoteAssetInstallMode;
  resourcePackages?: RemoteResourcePackageSelection;
}

export interface WSLConnectOptions {
  kind: "wsl";
  distro?: string;
  user?: string;
}

export interface DockerConnectOptions {
  kind: "docker";
  container: string;
}

export interface LanConnectOptions {
  kind: "lan";
  /**
   * 主端点地址（IPv4/域名，不含 ":"）。identity 只取 `host/port`，运行时可在
   * `endpoints` 里按公网 → LAN 顺序选可达端点；端点切换不改变 identity。
   */
  host: string;
  port: number;
  /** 主端点协议；缺省 http（旧记录兼容）。 */
  scheme?: "http" | "https";
  /** 主端点路径前缀（中继/反代，如 `/d/<tunnelId>`）；缺省根路径。 */
  path?: string;
  /**
   * 同一设备的全部可达端点（公网在前、LAN 在后）。缺省时按 `host/port` 构造单个端点。
   * 令牌仍走凭据服务/`token`，端点列表本身不含凭据。
   */
  endpoints?: LanPeerEndpoint[];
  /** 对端稳定标识：发现/配对时获得，用于历史条目与凭据键；不参与 identity。 */
  serverId?: string;
  serverName?: string;
  /**
   * 配对换取的一次性访问令牌：只在连接流程内透明传递，
   * 不写入 settings（见 stripRemoteTargetSecrets），落盘一律走凭据服务。
   */
  token?: string;
}

export type RemoteTarget =
  | SSHConnectOptions
  | WSLConnectOptions
  | DockerConnectOptions
  | LanConnectOptions;

/**
 * 解析 LAN 目标的连接端点：显式 `endpoints` 优先，否则用主端点构造一个。
 * 公网在前、LAN 在后，并去重；这是 UI、Host、探测/会话共用的唯一排序入口。
 */
export function resolveLanTargetEndpoints(target: LanConnectOptions): LanPeerEndpoint[] {
  const primary: LanPeerEndpoint = {
    scheme: target.scheme ?? "http",
    host: target.host.trim(),
    port: target.port,
    kind: (target.scheme ?? "http") === "https" ? "public" : "lan",
    ...(target.path ? { path: target.path } : {}),
  };
  const endpointKey = (endpoint: LanPeerEndpoint): string =>
    `${endpoint.scheme}://${endpoint.host}:${endpoint.port}${endpoint.path ?? ""}`;
  const candidates = target.endpoints?.length ? [...target.endpoints] : [];
  // 主端点不在列表里时补进去（旧调用方只填 host/port）。
  if (!candidates.some((endpoint) => endpointKey(endpoint) === endpointKey(primary))) {
    candidates.push(primary);
  }
  const seen = new Set<string>();
  return candidates
    .filter((endpoint) => {
      const key = endpointKey(endpoint);
      if (seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    })
    .sort((left, right) => {
      if (left.kind === right.kind) {
        return 0;
      }
      return left.kind === "public" ? -1 : 1;
    });
}

/**
 * 两个 `RemoteTarget` 是否指向同一个远端（bots 绑定/重连的唯一判等口径）。
 *
 * 只比较定位对端所需的 authority（口径见 `docs/specs/im-bot-remote-workspace-binding.md`）：
 *
 * | kind | 比较字段 |
 * | --- | --- |
 * | ssh | host（小写）、port（缺省 22）、username、privateKeyPath |
 * | wsl | distro（缺省 default）、user |
 * | docker | container |
 * | lan | host（小写）、port |
 *
 * secret（token、密码、私钥口令）与展示性字段（serverName、sshConfigAlias）**不参与比较**：
 * settings 快照与连接期 live target 的 secret 形态不同，比较它们必然把同一个对端判成两个。
 */
export function isSameRemoteTarget(left: RemoteTarget, right: RemoteTarget): boolean {
  if (left.kind !== right.kind) {
    return false;
  }
  switch (left.kind) {
    case "ssh":
      return (
        right.kind === "ssh" &&
        left.host.trim().toLowerCase() === right.host.trim().toLowerCase() &&
        (left.port ?? 22) === (right.port ?? 22) &&
        left.username.trim() === right.username.trim() &&
        (left.privateKeyPath?.trim() ?? "") === (right.privateKeyPath?.trim() ?? "")
      );
    case "wsl":
      return (
        right.kind === "wsl" &&
        (left.distro?.trim() || "default") === (right.distro?.trim() || "default") &&
        (left.user?.trim() ?? "") === (right.user?.trim() ?? "")
      );
    case "docker":
      return right.kind === "docker" && left.container.trim() === right.container.trim();
    case "lan":
      return (
        right.kind === "lan" &&
        left.host.trim().toLowerCase() === right.host.trim().toLowerCase() &&
        left.port === right.port
      );
    default:
      return false;
  }
}

/** 删除只应存在于当前连接流程中的 secret，供长期内存状态和跨进程回包使用。 */
export function stripRemoteTargetSecrets(target: RemoteTarget): RemoteTarget {
  if (target.kind === "ssh") {
    const {
      password: _password,
      privateKeyPassphrase: _privateKeyPassphrase,
      ...sanitized
    } = target;
    return sanitized;
  }

  if (target.kind === "lan") {
    const { token: _token, ...sanitized } = target;
    return sanitized;
  }

  return target;
}
