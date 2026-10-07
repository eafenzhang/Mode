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
  /** 局域网内对端 Mode 实例的地址（IPv4 或主机名，不含 ":"，与 identity 段规则一致）。 */
  host: string;
  port: number;
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
