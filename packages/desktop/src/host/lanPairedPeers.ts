import {
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  lanPeerMetaSchema,
  type LanPairedPeer,
  type LanPairedPeerWorkspaces,
  type LanPeerMeta,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

import { fetchLanPeerInfo, loadLanPeerToken } from "./lanRemoteAttach.js";

/**
 * 「我配对的对端」的凭据读写（Host 侧，Electron-free）。
 *
 * 这份逻辑原先内联在 Electron Host 的 IPC handler 里。Tauri 的 `lan-access` 频道需要
 * **同一套语义**（只读元数据、令牌不出现、删除让 token/meta 同生命周期收口），
 * 所以抽成唯一实现，两条 Host 线都从这里调用。
 */

export function parseLanPeerMeta(raw: string | null): LanPeerMeta | null {
  if (!raw) {
    return null;
  }
  try {
    const parsed = lanPeerMetaSchema.safeParse(JSON.parse(raw));
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}

/**
 * 枚举凭据里的对端记录：只读键名与元数据，不加载令牌本体。
 * 旧配对没有 meta 时保留一行（host 为空），至少还能删除。
 */
export async function listLanPairedPeers(
  credentials: ICredentialService,
): Promise<LanPairedPeer[]> {
  const keys = await credentials.list("lan:peer:");
  const peers: LanPairedPeer[] = [];
  for (const key of keys) {
    if (!key.startsWith("lan:peer:") || !key.endsWith(":token")) {
      continue;
    }
    const serverId = key.slice("lan:peer:".length, -":token".length);
    if (!serverId) {
      continue;
    }
    const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey(serverId)));
    peers.push(
      meta
        ? {
            serverId,
            host: meta.host,
            port: meta.port,
            ...(meta.name ? { name: meta.name } : {}),
            ...(meta.pairedAt ? { pairedAt: meta.pairedAt } : {}),
          }
        : { serverId, host: "", port: 0 },
    );
  }
  peers.sort((a, b) => (b.pairedAt ?? 0) - (a.pairedAt ?? 0));
  return peers;
}

/** 删除配对记录：令牌与元数据同生命周期，缺任意一个都不影响幂等删除。 */
export async function removeLanPairedPeerRecord(
  credentials: ICredentialService,
  serverId: string,
): Promise<void> {
  const trimmed = serverId.trim();
  if (!trimmed) {
    throw new Error("缺少 serverId");
  }
  await credentials.delete(buildLanPeerTokenKey(trimmed));
  await credentials.delete(buildLanPeerMetaKey(trimmed));
}

/**
 * 只改本机元数据里的展示名：对端不受影响，令牌/地址不动（下次连接照常）。
 * 没有元数据时不能假装成功——界面会显示一个没有落盘的名称。
 */
export async function renameLanPairedPeerRecord(
  credentials: ICredentialService,
  serverId: string,
  name: string,
): Promise<void> {
  const trimmedId = serverId.trim();
  if (!trimmedId) {
    throw new Error("缺少 serverId");
  }
  const trimmedName = name.trim();
  if (!trimmedName || trimmedName.length > 64) {
    throw new Error("名称长度必须为 1–64 个字符");
  }
  const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey(trimmedId)));
  if (!meta) {
    throw new Error("该配对缺少元数据，无法重命名");
  }
  await credentials.save(
    buildLanPeerMetaKey(trimmedId),
    JSON.stringify({ ...meta, name: trimmedName }),
  );
}

/** 用已存令牌访问对端 server-info；令牌缺失/失效都转成可读错误。 */
export async function getLanPairedPeerWorkspaces(
  credentials: ICredentialService,
  serverId: string,
): Promise<LanPairedPeerWorkspaces> {
  const trimmedId = serverId.trim();
  if (!trimmedId) {
    throw new Error("缺少 serverId");
  }
  const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey(trimmedId)));
  if (!meta) {
    throw new Error("该配对缺少地址信息，无法获取工作区");
  }
  const token = await loadLanPeerToken(credentials, trimmedId);
  if (!token) {
    throw new Error("尚未配对");
  }
  const info = await fetchLanPeerInfo({ host: meta.host, port: meta.port, token });
  return {
    ...(info.name ? { name: info.name } : {}),
    workspaces: info.workspaces,
  };
}
