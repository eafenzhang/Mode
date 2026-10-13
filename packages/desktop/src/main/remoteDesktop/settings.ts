/**
 * 被控远程桌面设置（main 侧唯一读写 API + 变更事件）。
 *
 * 唯一属主与持久化：真相在 host 进程的凭据存储（总开关 `remote-desktop:server-enabled`；
 * 逐设备授权挂在既有 lan client 记录的可选字段上，见 host/remoteDesktopServer.ts）。
 * main 这里只做「API + 变更通知」，不另立第二份配置（spec 状态所有者表 / AGENTS 集成约束）。
 * 读取是低频且要求新鲜（同意决策要立刻看到刚撤销的设备），因此不缓存，逐次经 host 现读。
 */
import {
  requestRemoteDesktopAuthorizeClient,
  requestRemoteDesktopRevokeClient,
  requestRemoteDesktopServerState,
  requestRemoteDesktopSetServerEnabled,
} from "./hostBridge.js";

export interface RemoteDesktopAuthorizedClient {
  id: string;
  label: string;
}

export interface RemoteDesktopServerState {
  enabled: boolean;
  authorizedClients: RemoteDesktopAuthorizedClient[];
}

type StateListener = (state: RemoteDesktopServerState) => void;

const listeners = new Set<StateListener>();

function publish(state: RemoteDesktopServerState): RemoteDesktopServerState {
  // Set 迭代器容忍迭代中取消订阅（删除当前元素），无需先复制快照。
  for (const listener of listeners) {
    try {
      listener(state);
    } catch {
      // 订阅方异常不得影响设置写入链。
    }
  }
  return state;
}

export function onRemoteDesktopServerStateChanged(listener: StateListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 读被控设置（经 host 现读；失败会 reject，调用方自行兜底成 fail-closed 展示）。 */
export async function readRemoteDesktopServerState(): Promise<RemoteDesktopServerState> {
  const state = await requestRemoteDesktopServerState();
  return publish(state);
}

export async function setRemoteDesktopServerEnabled(enabled: boolean): Promise<RemoteDesktopServerState> {
  const state = await requestRemoteDesktopSetServerEnabled(enabled);
  return publish(state);
}

export async function revokeRemoteDesktopClient(clientId: string): Promise<RemoteDesktopServerState> {
  const state = await requestRemoteDesktopRevokeClient(clientId);
  return publish(state);
}

/** 设置页授权开关：与同意窗「记住此设备」写同一记录字段（host 端同一收口）。 */
export async function authorizeRemoteDesktopClient(
  clientId: string,
): Promise<RemoteDesktopServerState> {
  const state = await requestRemoteDesktopAuthorizeClient(clientId);
  return publish(state);
}

/** 该设备是否已被记住（同意决策用；读取失败按未授权处理，fail-closed 弹同意窗）。 */
export async function isRemoteDesktopClientAuthorized(clientId: string): Promise<boolean> {
  try {
    const state = await readRemoteDesktopServerState();
    return state.authorizedClients.some((client) => client.id === clientId);
  } catch {
    return false;
  }
}
