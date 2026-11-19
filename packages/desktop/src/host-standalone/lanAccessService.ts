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

import type { V3DirectEndpoint } from "@mode/server";
import {
  startLanAccessServer,
  type LanAccessServerHandle,
  type StartLanAccessServerParams,
} from "../host/lanAccessServer.js";
import { resolveRemoteAssetDirs } from "./remoteAssets.js";
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
 * 本适配器只做四件事：
 *
 * 1. **单 owner 启动**：`ensureStarted` 是一条共享 Promise，并发 enable 不会起两个监听；
 * 2. **模式切换串行**：开关只在 `0.0.0.0`（LAN 暴露）与 `127.0.0.1`（本机服务常驻）
 *    之间切换，切换走「停旧 → 起新」并排队执行，避免交叠出双监听；
 * 3. **local-only 快照**：`getState` 在本机服务未启动时仍从凭据读出配对记录；
 * 4. **事件只发幂等快照**：`onDidChange` 在每次成功动作后发最新 `LanAccessState`。
 *
 * 它不该被 `createHttpServer` 看到：`startLanAccessServer` 拿到的是普通服务集合
 * （不含本频道），否则 LAN 对端就能调用管理面（docs/specs/tauri-renderer-host.md §21.2）。
 */

export interface LanAccessHostService extends ILanAccessService {
  dispose(): Promise<void>;
  /**
   * 远程桌面 server 的 client 记录写者：运行期唯一的内存所有者。
   * 本机服务未启动时返回 null（此时 RD 设置面按凭据读改写）。
   */
  getActiveServer(): LanAccessServerHandle | null;
  /**
   * 确保本机 HTTP/WS 可用（v3 工作区/远程桌面的 E2EE 隧道入口）。
   * LAN 模式直接复用；loopback 模式或尚未启动时以 loopback 起一份。
   * 不改变「允许局域网直连」开关值。
   */
  ensureLocalApi(): Promise<LanAccessServerHandle>;
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
  /** 显式监听端口（多实例/验收隔离）；缺省 `LAN_ACCESS_DEFAULT_PORT`。 */
  port?: number;
  /** 测试注入点：默认真实 UDP/Tailscale 探测。 */
  discover?: () => Promise<LanDiscoveredPeer[]>;
  /** 部署写入的内置中继配置；缺省由 startLanAccessServer 读 env。 */
  relay?: { baseUrl?: string; token?: string };
  /** v3 LAN 直连入口；闭包延迟绑定到 v3 服务。 */
  v3Direct?: V3DirectEndpoint;
  /** Web/手机远控静态资源目录；缺省不挂载静态资源。 */
  webRoot?: string;
  /**
   * 启动时按设置恢复监听模式：`lanAccessEnabled=true` → `0.0.0.0`（LAN 暴露），
   * 否则 → `127.0.0.1`（本机服务常驻，见 remote-control-unified-pairing.md R5）。
   * Host 入口保持默认 true；适配器单测传 false，避免在测试机绑定 45880/45879。
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

  /**
   * 配对类动作只在 LAN 暴露模式下可用。loopback 模式下 handle 也存在
   * （本机服务常驻，见 docs/specs/remote-control-unified-pairing.md R5），
   * 但局域网没开，必须按「未开启」失败而不是假装成功。
   */
  const requireLanHandle = (): LanAccessServerHandle => {
    if (!handle || !handle.getState().enabled) {
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

  /**
   * 生命周期串行化：模式切换是「停旧监听 → 起新监听」，与并发 enable / dispose
   * 交叠会出现双监听或把刚起来的模式又被停掉。所有会改变监听面的操作都排队执行。
   */
  let lifecycle: Promise<unknown> = Promise.resolve();
  const runExclusive = <T>(task: () => Promise<T>): Promise<T> => {
    const next = lifecycle.then(task, task);
    lifecycle = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };

  /**
   * 确保监听处于目标模式并已启动。
   * `localOnly=true` → `127.0.0.1`（本机服务，开关关闭态）；`false` → `0.0.0.0`（LAN 暴露）。
   * 模式不同先停旧再起新；模式相同直接复用，不会重复监听。
   */
  const ensureStarted = async (localOnly: boolean): Promise<LanAccessServerHandle> => {
    if (disposed) {
      throw new Error("局域网访问服务已释放");
    }
    const wantLan = !localOnly;
    if (handle && handle.getState().enabled === wantLan) {
      return handle;
    }
    if (handle) {
      const current = handle;
      handle = null;
      await current.dispose().catch((error: unknown) => {
        log(`lan_access_dispose_failed ${describeError(error)}`);
      });
    }
    if (!startPromise) {
      const mode = localOnly ? "local" : "lan";
      startPromise = (async () => {
        const remoteDesktop = options.getRemoteDesktop?.() ?? null;
        const started = await startServer({
          services,
          credentials: requireCredentials(),
          getWorkspaces: () => collectLanAccessWorkspaces(services),
          appVersion,
          machineName,
          localOnly,
          isRemoteDesktopServerEnabled: () => remoteDesktop?.isServerEnabled() ?? false,
          // 在线 web 会话数变化：把新快照推给订阅者（远控弹窗据此切换“等待/已连接”）。
          onWebSessionChange: () => {
            void publish().catch(() => undefined);
          },
          ...(options.webRoot ? { webRoot: options.webRoot } : {}),
          ...(options.relay ? { relay: options.relay } : {}),
          ...(options.v3Direct ? { v3Direct: options.v3Direct } : {}),
          ...(options.port ? { port: options.port } : {}),
          ...(remoteDesktop ? { remoteDesktop: remoteDesktop.getLanHooks() } : {}),
          // 手机经本 Host 发起的 SSH/Docker 连接与桌面 renderer 使用同一份资产源解析
          //（含构建期内置 CDN 与开发态 mock-cdn）；不传则服务端只能读 env。
          remoteAssetDirs: resolveRemoteAssetDirs(),
        });
        handle = started;
        log(`lan_access_started mode=${mode} port=${started.getLocalApiPort()}`);
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
      try {
        // 关闭不是停服：切到 loopback 模式，本机 HTTP/WS 继续服务 v3 工作区/远程桌面。
        await runExclusive(() => ensureStarted(!enabled));
      } catch (error) {
        // 开关是持久化事实：LAN 启动失败必须回写 false，否则下次启动会反复尝试且 UI 状态撒谎。
        await settingService.update({ lanAccessEnabled: false }).catch(() => undefined);
        log(`lan_access_start_failed ${describeError(error)}`);
        if (enabled) {
          // LAN 起不来（端口占用/防火墙）时尽量回落 loopback：远程控制不能因为
          // 局域网不可用而整体不可用；回落失败则保持无 handle，由 getState 报 disabled。
          await runExclusive(() => ensureStarted(true)).catch(() => undefined);
        }
        throw new Error(`局域网访问启动失败：${describeError(error)}`);
      }
      return publish();
    },

    async createPairCode(): Promise<LanAccessState> {
      return publishWith(requireLanHandle().createPairCode());
    },

    async removeClient(params: LanAccessRemoveClientParams): Promise<LanAccessState> {
      return publishWith(await requireLanHandle().removeClient(params.clientId ?? ""));
    },

    async resetTokens(): Promise<LanAccessState> {
      return publishWith(await requireLanHandle().resetTokens());
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

    ensureLocalApi: () =>
      runExclusive(async () => {
        // LAN 模式已有 handle：直接复用，绝不能为「确保本机可用」把 LAN 停掉。
        if (handle) {
          return handle;
        }
        return await ensureStarted(true);
      }),

    async dispose(): Promise<void> {
      if (disposed) {
        return;
      }
      disposed = true;
      onDidChangeEmitter.dispose();
      await runExclusive(() => stop());
    },
  };

  function publishWith(state: LanAccessState): LanAccessState {
    lastState = state;
    onDidChangeEmitter.fire(state);
    return state;
  }

  if (options.restoreOnStart !== false) {
    const settingService = services.getOptional(ISettingService);
    const settings = await settingService?.get().catch(() => null);
    // 本机服务常驻（R5）：开关关着也起 loopback，v3 工作区/远程桌面才能经 E2EE 隧道
    // 调用本机 API；开关开着才以 0.0.0.0 暴露局域网。
    const lanEnabled = settings?.lanAccessEnabled === true;
    try {
      await runExclusive(() => ensureStarted(!lanEnabled));
    } catch (error: unknown) {
      // 恢复失败不阻断 Host 启动；LAN 起不来时回写开关并回落 loopback，
      // 让远程控制保持可用而不是整体不可用。
      log(`lan_access_restore_failed ${describeError(error)}`);
      if (lanEnabled) {
        await settingService?.update({ lanAccessEnabled: false }).catch(() => undefined);
        await runExclusive(() => ensureStarted(true)).catch((fallbackError: unknown) => {
          log(`lan_access_local_restore_failed ${describeError(fallbackError)}`);
        });
      }
    }
  }
  // 首个订阅者拿到的是**当前**快照（不论监听是否开启）；读凭据失败不阻断 Host 启动。
  lastState = await readState().catch(() => null);

  // 内部生命周期方法不能被 ProxyChannel 暴露成 RPC command。
  return hideInternalServiceMembers(service, ["dispose", "getActiveServer", "ensureLocalApi"]);
}
