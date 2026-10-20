import type { Event } from "@mode/rpc";
import type {
  LanAccessPairResult,
  LanAccessState,
  LanDiscoveredPeer,
  LanPairPeerRequest,
  LanPairedPeer,
  LanPairedPeerWorkspaces,
} from "@mode/shared";

import { ServiceChannels } from "@mode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * 局域网管理面（LR-1，浏览器安全契约）。
 *
 * 状态所有者仍是 Host 进程里的 `LanAccessServerHandle` 与凭据/设置服务；renderer 只是读取
 * 快照并下发命令。实现（`packages/desktop/src/host-standalone/lanAccessService.ts`）依赖
 * Node/`@mode/server`，只能从 Host 侧装载，因此本文件只保留 interface 与 descriptor。
 *
 * **local-only**：这个频道只注册给窗口 renderer 的 `ChannelServer`，不得进入
 * `createHttpServer` 使用的服务集合（`pairPeer` 的返回值含长期令牌）。
 *
 * 所有参数用对象：位置参数在跨进程契约里无法演进（docs/specs/tauri-renderer-host.md §21.3）。
 */

export interface LanAccessSetEnabledParams {
  enabled: boolean;
}

export interface LanAccessRemoveClientParams {
  clientId: string;
}

export interface LanAccessServerIdParams {
  serverId: string;
}

export interface LanAccessRenamePairedPeerParams {
  serverId: string;
  /** 本机展示名；1–64 字符，不改对端记录。 */
  name: string;
}

export interface LanAccessSetPeerPublicEndpointParams {
  serverId: string;
  /** 公网 HTTPS 基础地址；null = 清除。 */
  publicBaseUrl: string | null;
}

export interface ILanAccessService {
  /** 监听未开启时 clients 仍从凭据读取（配对记录是持久化事实）。 */
  getState(): Promise<LanAccessState>;
  /** 开关监听并持久化 `lanAccessEnabled`；启动失败时抛错而不是返回假状态。 */
  setEnabled(params: LanAccessSetEnabledParams): Promise<LanAccessState>;
  /** 生成一次性配对码；监听未开启时抛错。 */
  createPairCode(): Promise<LanAccessState>;
  /** 撤销一个已配对客户端的令牌；监听未开启时抛错。 */
  removeClient(params: LanAccessRemoveClientParams): Promise<LanAccessState>;
  /** 撤销全部客户端令牌；监听未开启时抛错。 */
  resetTokens(): Promise<LanAccessState>;
  /** 广播探测局域网内实例（1.5s 窗口，已按 serverId+port 合并）。 */
  discoverPeers(): Promise<LanDiscoveredPeer[]>;
  /** 用对端配对码换取长期令牌并落凭据；返回值只给本机 renderer。 */
  pairPeer(request: LanPairPeerRequest): Promise<LanAccessPairResult>;
  /** 列出本机「我配对的对端」，只读元数据，不暴露令牌。 */
  listPairedPeers(): Promise<LanPairedPeer[]>;
  /** 用已存令牌读取对端工作区目录；失败时抛出可读错误。 */
  getPeerWorkspaces(params: LanAccessServerIdParams): Promise<LanPairedPeerWorkspaces>;
  /** 删除配对令牌与元数据；缺元数据也允许删除。 */
  removePairedPeer(params: LanAccessServerIdParams): Promise<void>;
  /** 只改本机元数据里的展示名；缺元数据时抛错。 */
  renamePairedPeer(params: LanAccessRenamePairedPeerParams): Promise<void>;
  /**
   * 新增/替换/清除某设备的公网 HTTPS 端点（令牌与 LAN 端点不动）。
   * 只接受 https；清除后没有其它端点时拒绝（调用方应改为删除配对）。
   */
  setPairedPeerPublicEndpoint(params: LanAccessSetPeerPublicEndpointParams): Promise<void>;
  /** 幂等状态快照；首个订阅者拿到最近一次快照。 */
  onDidChange: Event<LanAccessState>;
}

export const ILanAccessService = createServiceDescriptor<ILanAccessService>(
  ServiceChannels.LanAccess,
);
