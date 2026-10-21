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
