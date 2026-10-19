/* eslint-disable max-lines -- 设置/同意/主控/被控协调共享同一状态机与收口路径，拆散会让边界分散。 */
import { join } from "node:path";

import { Emitter } from "@mode/rpc";
import type {
  RemoteDesktopConsentRequest,
  RemoteDesktopConsentResponse,
  RemoteDesktopLocalDisplay,
  RemoteDesktopPeerCapability,
  RemoteDesktopServerState,
  RemoteDesktopSessionEvent,
  RemoteDesktopStartSessionResult,
} from "@mode/shared/remote-desktop";
import {
  ICredentialService,
  IRemoteDesktopService,
  type RemoteDesktopClientParams,
  type RemoteDesktopControlledState,
  type RemoteDesktopEndSessionParams,
  type RemoteDesktopPeerParams,
  type RemoteDesktopSendSignalParams,
  type RemoteDesktopSetServerEnabledParams,
  type RemoteDesktopStartSessionParams,
  type ServiceCollection,
} from "@mode/services";
import {
  getAppConfigDir,
  getCuaRawInputInjector,
  getCuaRdMediaController,
} from "@mode/services/node";

import type { LanAccessServerHandle } from "../host/lanAccessServer.js";
import {
  createRemoteDesktopHostController,
  type RemoteDesktopPeerSessionFailure,
  type RemoteDesktopPeerSessionOpen,
} from "../host/remoteDesktopController.js";
import {
  RemoteDesktopHostMessageTypes,
  type RemoteDesktopMessageFromHost,
  type RemoteDesktopMessageToHost,
} from "../host/remoteDesktopMessages.js";
import {
  createRemoteDesktopControlledCoordinator,
  type RemoteDesktopControlledCoordinator,
} from "../host/remoteDesktopControlledCoordinator.js";
import {
  openRemoteDesktopPeerSession,
  probeRemoteDesktopPeer,
} from "../host/remoteDesktopPeerClient.js";
import {
  createRemoteDesktopHostServer,
  type RemoteDesktopHostServer,
} from "../host/remoteDesktopServer.js";
import { hideInternalServiceMembers } from "./internalServiceMembers.js";

/**
 * `remote-desktop` 频道的 Host 实现（LR-2 设置面 + LR-3 主控会话/同意流，无 Electron）。
 *
 * 两个状态所有者都在 Host：
 * - 被控设置与同意：`RemoteDesktopHostServer`（message 命令是唯一写路径）；
 * - 主控会话：`createRemoteDesktopHostController`（单会话互斥、WS 信令、bye 收口）。
 *
 * 适配器不直接改凭据：设置命令翻译成 server 既有消息，主控会话经 `openSession` 打开对端票据。
 * 这样设置页、LAN 握手能力位、同意流写的是同一条路径（spec §21.1 的单所有者要求）。
 */

export interface RemoteDesktopHostService extends IRemoteDesktopService {
  /** LAN 握手用的能力位读取点（同步缓存）。 */
  isServerEnabled(): boolean;
  /** LAN 握手把信令/媒体 socket 交给同一 owner。 */
  getLanHooks(): RemoteDesktopHostServer["hooks"];
  /** 把同意结果或主控消息递回被控 server。 */
  handleMessage(message: RemoteDesktopMessageToHost): void;
  dispose(): Promise<void>;
}

export interface CreateRemoteDesktopHostServiceOptions {
  services: ServiceCollection;
  log: (line: string) => void;
  /** 运行期 LAN 服务端（lan client 记录的唯一内存写者）；无监听时返回 null。 */
  getActiveLanAccessServer?: () => LanAccessServerHandle | null;
  /** 测试注入点：替换 server 工厂（仍拿到真实的 `postToMain` 与依赖）。 */
  createServer?: (
    params: Parameters<typeof createRemoteDesktopHostServer>[0],
  ) => RemoteDesktopHostServer;
  /** 测试注入点：对端能力探测。 */
  probePeer?: (
    credentials: ICredentialService,
    serverId: string,
  ) => Promise<RemoteDesktopPeerCapability>;
  /** 测试注入点：主控对端会话打开（缺省用已存令牌 POST）。 */
  openSession?: (
    serverId: string,
    quality?: Parameters<typeof openRemoteDesktopPeerSession>[2],
    codec?: Parameters<typeof openRemoteDesktopPeerSession>[3],
  ) => Promise<RemoteDesktopPeerSessionOpen | RemoteDesktopPeerSessionFailure>;
  /** 测试注入：媒体密钥/盐（64 hex / 16 hex）。 */
  createKeySalt?: () => { key: string; salt: string };
  /** 编码能力缓存路径覆写（缺省 = app config 下的 rd-encoder-capability.json）。 */
  encoderCapabilityPath?: string;
  /** 同意窗超时（缺省 60s；与 Electron 同意窗一致，必须早于被控 server 的 75s 兜底）。 */
  consentTimeoutMs?: number;
  /** 非设置面消息（同意请求、媒体状态等）的旁路观察口；不参与状态写。 */
  onHostMessage?: (message: RemoteDesktopMessageFromHost) => void;
}

/** host 侧状态请求的统一预算（与 UI 可见请求同口径）。 */
const REQUEST_TIMEOUT_MS = 10_000;
const DEFAULT_CONSENT_TIMEOUT_MS = 60_000;

function describeArgs(args: unknown[]): string {
  return args.map((value) => (value instanceof Error ? value.message : String(value))).join(" ");
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export async function createRemoteDesktopHostService(
  options: CreateRemoteDesktopHostServiceOptions,
): Promise<RemoteDesktopHostService> {
  const { services, log } = options;
  const credentials = services.getOptional(ICredentialService);
  if (!credentials) {
    throw new Error("凭据服务不可用，无法初始化远程桌面被控设置");
  }

  const pendingState = new Map<string, (message: ServerStateResponse) => void>();
  let nextRequestId = 0;

  let pendingConsentClientId: string | null = null;
  let pendingConsentRequest: RemoteDesktopConsentRequest | null = null;
  let consentTimer: ReturnType<typeof setTimeout> | null = null;
  const consentTimeoutMs = options.consentTimeoutMs ?? DEFAULT_CONSENT_TIMEOUT_MS;
  /** renderer 推来的显示器快照（D7：Host 不直接枚举显示器）。 */
  let localDisplays: RemoteDesktopLocalDisplay[] = [];
  /** 被控协调器要等 server 建好后才能注入；postToMain 先按可选调用。 */
  let coordinator: RemoteDesktopControlledCoordinator | null = null;
  /** 非幂等信令：只发给当前订阅者，不缓冲。 */
  const sessionEventEmitter = new Emitter<RemoteDesktopSessionEvent>();
  /**
   * 同意请求是幂等事实：首个订阅者必须补上仍在等待的那一条。
   * （ProxyChannel 的 buffer 只在首次 subscribe 时才订阅源事件，裸 Emitter 早发会丢。）
   */
  const consentEmitter = new Emitter<RemoteDesktopConsentRequest>({
    onWillAddFirstListener: () => {
      if (pendingConsentRequest) {
        consentEmitter.fire(pendingConsentRequest);
      }
    },
  });
  /** 被控状态（指示条）：幂等，首个订阅者补当前值。 */
  let currentControlled = false;
  const controlledEmitter = new Emitter<RemoteDesktopControlledState>({
    onWillAddFirstListener: () => {
      controlledEmitter.fire({ controlled: currentControlled });
    },
  });

  const postToMain = (message: RemoteDesktopMessageFromHost): void => {
    if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult) {
      const resolve = pendingState.get(message.requestId);
      if (resolve) {
        pendingState.delete(message.requestId);
        resolve(message);
        return;
      }
    }
    if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest) {
      // 新会话：清掉上一会话的媒体状态（密钥/盐、是否已开流）。
      coordinator?.reset();
      // 不阻塞 postToMain：已记住设备要现读授权列表（异步），未记住则进入 60s 等待。
      void handleConsentRequest(message.clientId, message.label);
      options.onHostMessage?.(message);
      return;
    }
    if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost) {
      coordinator?.handleControllerSignal(message.message);
      options.onHostMessage?.(message);
      return;
    }
    if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus) {
      coordinator?.handleMediaStatus({
        frozen: message.frozen,
        ...(message.reason ? { reason: message.reason } : {}),
        ...(message.unavailable ? { unavailable: message.unavailable } : {}),
      });
      options.onHostMessage?.(message);
      return;
    }
    if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed) {
      coordinator?.handleSessionClosed();
      options.onHostMessage?.(message);
      return;
    }
    options.onHostMessage?.(message);
  };

  const injectRawInput = getCuaRawInputInjector(services);
  const callRdMedia = getCuaRdMediaController(services);
  const createServer = options.createServer ?? createRemoteDesktopHostServer;
  const server = createServer({
    credentials,
    logger: {
      info: (...args: unknown[]) => log(describeArgs(args)),
      warn: (...args: unknown[]) => log(`warn ${describeArgs(args)}`),
    },
    postToMain,
    getActiveLanAccessServer: options.getActiveLanAccessServer ?? (() => null),
    ...(injectRawInput ? { injectRawInput: (events) => injectRawInput(events) } : {}),
    ...(callRdMedia ? { callRdMedia: (method, params) => callRdMedia(method, params) } : {}),
    encoderCapabilityPath:
      options.encoderCapabilityPath ?? join(getAppConfigDir(), "rd-encoder-capability.json"),
  });
  await server.loadServerState();

  const controlledSubscription = server.onDidChangeControlledState((controlled) => {
    currentControlled = controlled;
    controlledEmitter.fire({ controlled });
  });
  currentControlled = server.isControlled();

  coordinator = createRemoteDesktopControlledCoordinator({
    getDisplays: () => localDisplays,
    sendSignal: (message) =>
      server.handleMessage({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost,
        message,
      }),
    sendHostMessage: (message) => server.handleMessage(message),
    logger: {
      info: (...args: unknown[]) => log(describeArgs(args)),
      warn: (...args: unknown[]) => log(`warn ${describeArgs(args)}`),
    },
    ...(options.createKeySalt ? { createKeySalt: options.createKeySalt } : {}),
  });

  let lastState: RemoteDesktopServerState | null = null;
  const onDidChangeEmitter = new Emitter<RemoteDesktopServerState>({
    onWillAddFirstListener: () => {
      if (lastState) {
        onDidChangeEmitter.fire(lastState);
      }
    },
  });

  const toState = (message: ServerStateResponse): RemoteDesktopServerState => ({
    enabled: message.enabled,
    authorizedClients: message.authorizedClients,
    encoderCapability: message.encoderCapability ?? "unknown",
  });

  /** 发一条设置命令并等 server 回同 requestId 的状态快照。 */
  const requestState = (
    build: (requestId: string) => RemoteDesktopMessageToHost,
  ): Promise<RemoteDesktopServerState> =>
    new Promise<RemoteDesktopServerState>((resolve, reject) => {
      const requestId = `rd-${nextRequestId++}`;
      const timer = setTimeout(() => {
        pendingState.delete(requestId);
        reject(new Error("远程桌面状态请求超时（Host 未回帖）"));
      }, REQUEST_TIMEOUT_MS);
      pendingState.set(requestId, (message) => {
        clearTimeout(timer);
        if (!message.ok) {
          reject(new Error(message.error ?? "远程桌面状态请求失败"));
          return;
        }
        resolve(toState(message));
      });
      try {
        server.handleMessage(build(requestId));
      } catch (error) {
        clearTimeout(timer);
        pendingState.delete(requestId);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });

  const publish = (state: RemoteDesktopServerState): RemoteDesktopServerState => {
    lastState = state;
    onDidChangeEmitter.fire(state);
    return state;
  };

  const requireClientId = (raw: string): string => {
    const clientId = raw?.trim() ?? "";
    if (!clientId) {
      throw new Error("clientId 不能为空");
    }
    return clientId;
  };

  const clearConsentTimer = (): void => {
    if (consentTimer) {
      clearTimeout(consentTimer);
      consentTimer = null;
    }
  };

  const sendConsentResult = (response: RemoteDesktopConsentResponse): void => {
    server.handleMessage({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
      clientId: response.clientId,
      decision: response.decision,
      remember: response.remember,
      ...(response.reason ? { reason: response.reason } : {}),
    });
  };

  /**
   * 同意入口（被控侧）：已记住的设备直通；否则发事件并等 60s。
   * 被控 server 自己还有 75s 兜底；这里先到点，主控拿到的是 `consent_timeout`。
   */
  async function handleConsentRequest(clientId: string, label: string): Promise<void> {
    const state = await requestState((requestId) => ({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest,
      requestId,
    })).catch(() => null);
    if (state?.authorizedClients.some((client) => client.id === clientId)) {
      log(`rd_consent_authorized clientId=${clientId}`);
      pendingConsentRequest = null;
      sendConsentResult({ clientId, decision: "allow", remember: true });
      return;
    }
    clearConsentTimer();
    pendingConsentClientId = clientId;
    pendingConsentRequest = { clientId, label };
    consentTimer = setTimeout(() => {
      if (pendingConsentClientId !== clientId) {
        return;
      }
      pendingConsentClientId = null;
      pendingConsentRequest = null;
      consentTimer = null;
      log(`rd_consent_timeout clientId=${clientId}`);
      sendConsentResult({ clientId, decision: "deny", remember: false, reason: "timeout" });
    }, consentTimeoutMs);
    consentEmitter.fire(pendingConsentRequest);
  }

  const readPeerCapability = async (serverId: string): Promise<RemoteDesktopPeerCapability> => {
    const trimmed = serverId?.trim() ?? "";
    if (!trimmed) {
      return { remoteDesktop: false, remoteDesktopNative: false, online: false };
    }
    const probe = options.probePeer ?? probeRemoteDesktopPeer;
    try {
      return await probe(credentials, trimmed);
    } catch (error) {
      // 探测失败按离线展示（不猜能力位），与 Electron main 的 IPC 语义一致。
      log(`rd_peer_probe_failed ${describeError(error)}`);
      return { remoteDesktop: false, remoteDesktopNative: false, online: false };
    }
  };

  const controller = createRemoteDesktopHostController({
    openSession:
      options.openSession ??
      ((serverId, quality, codec) =>
        openRemoteDesktopPeerSession(credentials, serverId, quality, codec)),
    logger: {
      info: (...args: unknown[]) => log(describeArgs(args)),
      warn: (...args: unknown[]) => log(`warn ${describeArgs(args)}`),
    },
    push: (event) => sessionEventEmitter.fire(event),
  });

  const service: RemoteDesktopHostService = {
    getServerState: async () => {
      const state = await requestState((requestId) => ({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest,
        requestId,
      }));
      // 只更新快照，不发事件：读不是变更。
      lastState = state;
      return state;
    },

    async setServerEnabled(
      params: RemoteDesktopSetServerEnabledParams,
    ): Promise<RemoteDesktopServerState> {
      const enabled = params.enabled === true;
      return publish(
        await requestState((requestId) => ({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled,
          requestId,
          enabled,
        })),
      );
    },

    async authorizeClient(params: RemoteDesktopClientParams): Promise<RemoteDesktopServerState> {
      const clientId = requireClientId(params.clientId);
      return publish(
        await requestState((requestId) => ({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopAuthorizeClient,
          requestId,
          clientId,
        })),
      );
    },

    async revokeClient(params: RemoteDesktopClientParams): Promise<RemoteDesktopServerState> {
      const clientId = requireClientId(params.clientId);
      return publish(
        await requestState((requestId) => ({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient,
          requestId,
          clientId,
        })),
      );
    },

    getPeerCapability: (params: RemoteDesktopPeerParams) => readPeerCapability(params.serverId),

    getControlledState: async (): Promise<RemoteDesktopControlledState> => ({
      controlled: server.isControlled(),
    }),
    onDidChangeControlledState: controlledEmitter.event,

    async startSession(
      params: RemoteDesktopStartSessionParams,
    ): Promise<RemoteDesktopStartSessionResult> {
      const serverId = params.serverId?.trim() ?? "";
      if (!serverId) {
        return { error: "internal" };
      }
      // 产品规则 9：本机被控中（含同意等待）不可发起，防双向互控的输入串台。
      if (server.isControlled()) {
        return { error: "locally_controlled" };
      }
      return controller.start({ serverId, quality: params.quality, codec: params.codec });
    },

    async sendSignal(params: RemoteDesktopSendSignalParams): Promise<void> {
      controller.sendSignal(params.message);
    },

    async endSession(params?: RemoteDesktopEndSessionParams): Promise<void> {
      await controller.end(params?.reason);
    },

    async respondConsent(params: RemoteDesktopConsentResponse): Promise<void> {
      const clientId = requireClientId(params.clientId);
      if (pendingConsentClientId !== clientId) {
        throw new Error("没有待处理的同意请求");
      }
      pendingConsentClientId = null;
      pendingConsentRequest = null;
      clearConsentTimer();
      sendConsentResult({ ...params, clientId });
    },

    async setLocalDisplays(params: { displays: RemoteDesktopLocalDisplay[] }): Promise<void> {
      const displays = (params.displays ?? [])
        .filter(
          (display) =>
            display &&
            typeof display.displayId === "string" &&
            display.displayId.trim().length > 0 &&
            Number.isFinite(display.xPx) &&
            Number.isFinite(display.yPx) &&
            Number.isFinite(display.widthPx) &&
            Number.isFinite(display.heightPx) &&
            display.widthPx > 0 &&
            display.heightPx > 0,
        )
        .map((display) => ({
          displayId: display.displayId.trim(),
          label: typeof display.label === "string" ? display.label : "",
          xPx: Math.trunc(display.xPx),
          yPx: Math.trunc(display.yPx),
          widthPx: Math.max(1, Math.trunc(display.widthPx)),
          heightPx: Math.max(1, Math.trunc(display.heightPx)),
          scaleFactor:
            Number.isFinite(display.scaleFactor) && display.scaleFactor > 0
              ? display.scaleFactor
              : 1,
          primary: display.primary === true,
        }));
      if (displays.length > 0 && !displays.some((display) => display.primary)) {
        displays[0]!.primary = true;
      }
      localDisplays = displays;
      log(`rd_local_displays count=${displays.length}`);
    },

    onDynamicSessionEvent: (_sessionId?: string) => sessionEventEmitter.event,
    onConsentRequest: consentEmitter.event,
    onDidChangeServerState: onDidChangeEmitter.event,

    isServerEnabled: () => server.isServerEnabled(),
    getLanHooks: () => server.hooks,
    handleMessage: (message) => server.handleMessage(message),

    async dispose(): Promise<void> {
      clearConsentTimer();
      if (pendingConsentClientId) {
        // Host 退出：拒绝挂起的同意请求，让对端立刻失败而不是等 75s 兜底。
        const clientId = pendingConsentClientId;
        pendingConsentClientId = null;
        pendingConsentRequest = null;
        try {
          sendConsentResult({ clientId, decision: "deny", remember: false, reason: "user" });
        } catch {
          // server 可能已先行释放。
        }
      }
      // 主控会话先带出 bye，再收被控 server 与事件面。
      await controller.dispose();
      server.dispose();
      sessionEventEmitter.dispose();
      consentEmitter.dispose();
      controlledSubscription.dispose();
      controlledEmitter.dispose();
      onDidChangeEmitter.dispose();
      // 迟到的回帖不再有人等。
      for (const resolve of pendingState.values()) {
        resolve({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
          requestId: "disposed",
          ok: false,
          enabled: false,
          authorizedClients: [],
          encoderCapability: "unknown",
          error: "远程桌面服务已释放",
        });
      }
      pendingState.clear();
    },
  };
  // 内部生命周期方法不能被 ProxyChannel 暴露成 RPC command。
  return hideInternalServiceMembers(service, [
    "dispose",
    "isServerEnabled",
    "getLanHooks",
    "handleMessage",
  ]);
}

/** 与 host→main 的 `ServerStateResult` 消息同形的局部类型（不导出运行时 schema）。 */
interface ServerStateResponse {
  type: typeof RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult;
  requestId: string;
  ok: boolean;
  enabled: boolean;
  authorizedClients: Array<{ id: string; label: string }>;
  encoderCapability?: RemoteDesktopServerState["encoderCapability"];
  error?: string;
}
