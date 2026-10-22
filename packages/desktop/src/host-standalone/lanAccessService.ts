import { hostname } from "node:os";

import { Emitter } from "@mode/rpc";
import type {
  LanAccessPairResult,
  LanAccessState,
  LanDiscoveredPeer,
  LanPairPeerRequest,
  LanPairedPeerWorkspaces,
} from "@mode/shared";
import { MODE_VERSION } from "@mode/shared";
import {
  ICredentialService,
  ILanAccessService,
  ISettingService,
  type LanAccessRemoveClientParams,
  type LanAccessRenamePairedPeerParams,
  type LanAccessServerIdParams,
  type LanAccessSetEnabledParams,
  type ServiceCollection,
} from "@mode/services";

import {
  startLanAccessServer,
  type LanAccessServerHandle,
  type StartLanAccessServerParams,
} from "../host/lanAccessServer.js";
import { readDisabledLanAccessState } from "../host/lanAccessRecords.js";
import { discoverLanPeers } from "../host/lanDiscovery.js";
import {
  getLanPairedPeerWorkspaces,
  listLanPairedPeers,
  removeLanPairedPeerRecord,
  renameLanPairedPeerRecord,
} from "../host/lanPairedPeers.js";
import { pairLanPeer } from "../host/lanRemoteAttach.js";
import { collectLanAccessWorkspaces } from "../host/lanWorkspaceList.js";
import { hideInternalServiceMembers } from "./internalServiceMembers.js";

/**
 * `lan-access` 频道的 Host 实现（LR-1，无 Electron）。
 *
 * 状态所有者全部沿用既有实现：`LanAccessServerHandle` 持有监听/配对码/client 记录，
 * setting 服务持有 `lanAccessEnabled`，credential 服务持有对端令牌与元数据。
 * 本适配器只做三件事：
 *
 * 1. **单 owner 启动**：`ensureStarted` 是一条共享 Promise，并发 enable 不会起两个监听；
 * 2. **local-only 快照**：`getState` 在监听关闭时仍从凭据读出配对记录；
 * 3. **事件只发幂等快照**：`onDidChange` 在每次成功动作后发最新 `LanAccessState`。
 *
 * 它不该被 `createHttpServer` 看到：`startLanAccessServer` 拿到的是普通服务集合
 * （不含本频道），否则 LAN 对端就能调用管理面（docs/specs/tauri-renderer-host.md §21.2）。
 */

export interface LanAccessHostService extends ILanAccessService {
  dispose(): Promise<void>;
  /**
   * 远程桌面 server 的 client 记录写者：运行期唯一的内存所有者。
   * 监听未开启时返回 null（此时 RD 设置面按凭据读改写）。
   */
  getActiveServer(): LanAccessServerHandle | null;
}

export interface CreateLanAccessHostServiceOptions {
  services: ServiceCollection;
  log: (line: string) => void;
  appVersion?: string;
  machineName?: string;
  /**
   * 远程桌面被控 server（LR-2）：握手时现取能力位，并把信令/媒体 socket 交给同一 owner。
   * 缺省 = 未装配远程桌面（能力位 false、socket 一律关闭）。
   */
  getRemoteDesktop?: () => RemoteDesktopLanBridge | null;
  /** 测试注入点：默认 `startLanAccessServer`。 */
  startServer?: (params: StartLanAccessServerParams) => Promise<LanAccessServerHandle>;
  /** 测试注入点：默认真实 UDP/Tailscale 探测。 */
  discover?: () => Promise<LanDiscoveredPeer[]>;
  /** 部署写入的内置中继配置；缺省由 startLanAccessServer 读 env。 */
  relay?: { baseUrl?: string; token?: string };
  /** Web/手机远控静态资源目录；缺省不挂载静态资源。 */
  webRoot?: string;
  /**
   * 启动时按设置恢复监听。Host 入口保持默认 true；适配器单测传 false，
   * 避免在测试机绑定 45880/45879。
   */
  restoreOnStart?: boolean;
}

/** LAN 握手只需要远程桌面 server 的这两个面（避免把整个 server 接口塞进适配器）。 */
export interface RemoteDesktopLanBridge {
  isServerEnabled(): boolean;
  getLanHooks(): NonNullable<StartLanAccessServerParams["remoteDesktop"]>;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export async function createLanAccessHostService(
  options: CreateLanAccessHostServiceOptions,
): Promise<LanAccessHostService> {
  const { services, log } = options;
  const appVersion = options.appVersion ?? MODE_VERSION;
  const machineName = options.machineName ?? (hostname() || "Mode");
  const startServer = options.startServer ?? startLanAccessServer;
  const discover = options.discover ?? (() => discoverLanPeers());

  let handle: LanAccessServerHandle | null = null;
  /** 单一启动 Promise：所有并发 enable 共用同一次监听。 */
  let startPromise: Promise<LanAccessServerHandle> | null = null;
  let disposed = false;
  /** 最近一次快照：首个订阅者只补这一条（D5：幂等状态不做历史重放）。 */
  let lastState: LanAccessState | null = null;
  const onDidChangeEmitter = new Emitter<LanAccessState>({
    onWillAddFirstListener: () => {
      if (lastState) {
        onDidChangeEmitter.fire(lastState);
      }
    },
  });

  const requireCredentials = (): ICredentialService => {
    const credentials = services.getOptional(ICredentialService);
    if (!credentials) {
      throw new Error("凭据服务不可用，无法管理局域网配对");
    }
    return credentials;
  };

  const requireHandle = (): LanAccessServerHandle => {
    if (!handle) {
      throw new Error("局域网访问未开启");
    }
    return handle;
  };

  const readState = async (): Promise<LanAccessState> => {
    const state = handle
      ? handle.getState()
      : await readDisabledLanAccessState(requireCredentials());
    lastState = state;
    return state;
  };

  const publish = async (): Promise<LanAccessState> => {
    const state = await readState();
    onDidChangeEmitter.fire(state);
    return state;
  };

  const ensureStarted = async (): Promise<LanAccessServerHandle> => {
    if (disposed) {
      throw new Error("局域网访问服务已释放");
    }
    if (handle) {
      return handle;
    }
    if (!startPromise) {
      startPromise = (async () => {
        const remoteDesktop = options.getRemoteDesktop?.() ?? null;
        const started = await startServer({
          services,
          credentials: requireCredentials(),
          getWorkspaces: () => collectLanAccessWorkspaces(services),
          appVersion,
          machineName,
          isRemoteDesktopServerEnabled: () => remoteDesktop?.isServerEnabled() ?? false,
          // 在线 web 会话数变化：把新快照推给订阅者（远控弹窗据此切换“等待/已连接”）。
          onWebSessionChange: () => {
            void publish().catch(() => undefined);
          },
          ...(options.webRoot ? { webRoot: options.webRoot } : {}),
          ...(options.relay ? { relay: options.relay } : {}),
          ...(remoteDesktop ? { remoteDesktop: remoteDesktop.getLanHooks() } : {}),
        });
        handle = started;
        log(`lan_access_started port=${started.getState().port ?? "?"}`);
        return started;
      })().finally(() => {
        startPromise = null;
      });
    }
    return startPromise;
  };

  const stop = async (): Promise<void> => {
    // 启动中收到关闭：先等启动落定，再关刚起来的监听，避免「关掉后又冒出一个」。
    const pending = startPromise;
    if (pending) {
      await pending.catch(() => undefined);
    }
    const current = handle;
    handle = null;
    if (!current) {
      return;
    }
    await current.dispose().catch((error: unknown) => {
      log(`lan_access_dispose_failed ${describeError(error)}`);
    });
  };

  const service: LanAccessHostService = {
    getState: readState,

    async setEnabled(params: LanAccessSetEnabledParams): Promise<LanAccessState> {
      const enabled = params.enabled === true;
      const settingService = services.getOptional(ISettingService);
      if (!settingService) {
        throw new Error("设置服务不可用，无法持久化局域网开关");
      }
      await settingService.update({ lanAccessEnabled: enabled });
      if (enabled) {
        try {
          await ensureStarted();
        } catch (error) {
          // 开关是持久化事实：启动失败必须回写 false，否则下次启动会反复尝试且 UI 状态撒谎。
          await settingService.update({ lanAccessEnabled: false }).catch(() => undefined);
          log(`lan_access_start_failed ${describeError(error)}`);
          throw new Error(`局域网访问启动失败：${describeError(error)}`);
        }
      } else {
        await stop();
      }
      return publish();
    },

    async createPairCode(): Promise<LanAccessState> {
      return publishWith(requireHandle().createPairCode());
    },

    async removeClient(params: LanAccessRemoveClientParams): Promise<LanAccessState> {
      return publishWith(await requireHandle().removeClient(params.clientId ?? ""));
    },

    async resetTokens(): Promise<LanAccessState> {
      return publishWith(await requireHandle().resetTokens());
    },

    discoverPeers: () => discover(),

    async pairPeer(request: LanPairPeerRequest): Promise<LanAccessPairResult> {
      return pairLanPeer({
        credentials: requireCredentials(),
        host: request.host,
        port: request.port,
        code: request.code,
        ...(request.label ? { label: request.label } : {}),
        ...(request.scheme ? { scheme: request.scheme } : {}),
        ...(request.path ? { path: request.path } : {}),
      });
    },

    listPairedPeers: () => listLanPairedPeers(requireCredentials()),

    getPeerWorkspaces(params: LanAccessServerIdParams): Promise<LanPairedPeerWorkspaces> {
      return getLanPairedPeerWorkspaces(requireCredentials(), params.serverId);
    },

    async removePairedPeer(params: LanAccessServerIdParams): Promise<void> {
      await removeLanPairedPeerRecord(requireCredentials(), params.serverId);
    },

    async renamePairedPeer(params: LanAccessRenamePairedPeerParams): Promise<void> {
      await renameLanPairedPeerRecord(requireCredentials(), params.serverId, params.name);
    },

    onDidChange: onDidChangeEmitter.event,

    getActiveServer: () => handle,

    async dispose(): Promise<void> {
      if (disposed) {
        return;
      }
      disposed = true;
      onDidChangeEmitter.dispose();
      await stop();
    },
  };

  function publishWith(state: LanAccessState): LanAccessState {
    lastState = state;
    onDidChangeEmitter.fire(state);
    return state;
  }

  if (options.restoreOnStart !== false) {
    const settings = await services
      .getOptional(ISettingService)
      ?.get()
      .catch(() => null);
    if (settings?.lanAccessEnabled) {
      await ensureStarted().catch((error: unknown) => {
        // 恢复失败不阻断 Host 启动：状态回 disabled，UI 会看到开关未生效。
        log(`lan_access_restore_failed ${describeError(error)}`);
      });
    }
  }
  // 首个订阅者拿到的是**当前**快照（不论监听是否开启）；读凭据失败不阻断 Host 启动。
  lastState = await readState().catch(() => null);

  // 内部生命周期方法不能被 ProxyChannel 暴露成 RPC command。
  return hideInternalServiceMembers(service, ["dispose", "getActiveServer"]);
}
