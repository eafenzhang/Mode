/**
 * 主控方向的对端能力探测（host 侧）。
 *
 * 为什么放在 host 而不在 main：对端长期令牌保存在凭据服务（`lan:peer:<serverId>:token`），
 * host 是本机 LAN 客户端凭据的既有持有者；令牌只随本模块的出站请求交给对端，不写日志、
 * 不进 renderer。
 *
 * 系统 RDP 方案下这里只剩「探对端能力位与在线状态」：应用不再请求对端建立任何会话——
 * 连接是主控本地生成 `.rdp` 并启动 mstsc，被控端应用完全不参与
 * （docs/specs/remote-desktop-system-rdp.md 架构节）。
 */
import { buildLanPeerMetaKey, buildLanPeerTokenKey, lanPeerMetaSchema } from "@mode/shared";
import type { SystemRdpCapability } from "@mode/shared";
import type { ICredentialService } from "@mode/services";
import { fetchLanPeerInfo } from "./lanRemoteAttach.js";

export interface RemoteDesktopPeerInfo {
  online: boolean;
  /** 对端 `capabilities.remoteDesktop`（提示性共享位）；缺键 = false。 */
  remoteDesktop: boolean;
  /**
   * 对端系统 RDP 真实状态；缺键 = 状态未知（旧对端或对端探测未就绪）。
   * 主控**不得**据此置灰——状态未知仍允许发起，真不支持时由 mstsc 自身报错。
   */
  systemRdp?: SystemRdpCapability;
  name?: string;
}

interface PeerTarget {
  host: string;
  port: number;
  token?: string;
}

async function readPeerTarget(
  credentials: ICredentialService,
  serverId: string,
): Promise<PeerTarget | null> {
  const rawMeta = await credentials.load(buildLanPeerMetaKey(serverId));
  if (!rawMeta) {
    return null;
  }
  let meta: unknown;
  try {
    meta = JSON.parse(rawMeta);
  } catch {
    return null;
  }
  const parsed = lanPeerMetaSchema.safeParse(meta);
  if (!parsed.success) {
    return null;
  }
  const token = (await credentials.load(buildLanPeerTokenKey(serverId)))?.trim();
  return {
    host: parsed.data.host,
    port: parsed.data.port,
    ...(token ? { token } : {}),
  };
}

/** 能力位探测：任何失败（未配对/离线/旧版）都归为 online:false，不抛给调用方。 */
export async function probeRemoteDesktopPeer(
  credentials: ICredentialService,
  serverId: string,
): Promise<RemoteDesktopPeerInfo> {
  const target = await readPeerTarget(credentials, serverId);
  if (!target) {
    return { online: false, remoteDesktop: false };
  }
  try {
    const info = await fetchLanPeerInfo({
      host: target.host,
      port: target.port,
      ...(target.token ? { token: target.token } : {}),
    });
    const systemRdp = info.capabilities?.systemRdp;
    return {
      online: true,
      // 缺键 = 未共享为可远控设备（提示性开关关或旧对端）。
      remoteDesktop: info.capabilities?.remoteDesktop === true,
      ...(systemRdp ? { systemRdp } : {}),
      ...(info.name ? { name: info.name } : {}),
    };
  } catch {
    return { online: false, remoteDesktop: false };
  }
}
