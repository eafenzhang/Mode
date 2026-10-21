/**
 * LAN 已配对客户端记录的读写口（凭据服务 JSON 数组）。
 *
 * 唯一真相 = `lan:access:clients` 键与这份字段集合；LAN 服务端、远程桌面逐设备
 * 授权、监听关闭时的设置页快照都从这里读，避免多处复刻记录形状。
 */
import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  type LanAccessClientRecord,
  type LanAccessState,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

export interface LanAccessClientRecordWithRemoteDesktop extends LanAccessClientRecord {
  remoteDesktopAuthorized?: boolean;
}

export async function readLanAccessClientRecords(
  credentials: ICredentialService,
): Promise<LanAccessClientRecordWithRemoteDesktop[]> {
  const raw = await credentials.load(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY);
  if (!raw) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed
      .filter((item): item is LanAccessClientRecordWithRemoteDesktop => {
        if (!item || typeof item !== "object") {
          return false;
        }
        const record = item as Partial<LanAccessClientRecordWithRemoteDesktop>;
        return typeof record.id === "string" && record.id.length > 0;
      })
      .map((record) => ({
        id: record.id,
        label: typeof record.label === "string" ? record.label : "",
        createdAt: typeof record.createdAt === "number" ? record.createdAt : Date.now(),
        lastUsedAt: typeof record.lastUsedAt === "number" ? record.lastUsedAt : null,
        ...(typeof record.lastWorkspacePath === "string" && record.lastWorkspacePath
          ? { lastWorkspacePath: record.lastWorkspacePath }
          : {}),
        // 远程桌面逐设备授权：只认显式 true（缺键/旧数据 = 未授权），写回时必须保留。
        ...(record.remoteDesktopAuthorized === true ? { remoteDesktopAuthorized: true } : {}),
      }));
  } catch {
    return [];
  }
}

/** 写回记录数组（仅无活 handle 的远程桌面授权写路径使用；见 remoteDesktopServer.ts）。 */
export async function writeLanAccessClientRecords(
  credentials: ICredentialService,
  records: LanAccessClientRecordWithRemoteDesktop[],
): Promise<void> {
  await credentials.save(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY, JSON.stringify(records));
}

/**
 * 监听未开启时的状态快照（纯读取，不依赖服务端在跑）：
 * clients 从凭据读取——配对记录与逐设备授权是持久化事实，监听开关只管
 * 「现在能不能连进来」。port/addresses/pairCode 为空。
 */
export async function readDisabledLanAccessState(
  credentials: ICredentialService,
): Promise<LanAccessState> {
  return {
    enabled: false,
    port: null,
    addresses: [],
    pairCode: null,
    clients: await readLanAccessClientRecords(credentials),
    publicRelayUrl: null,
  };
}
