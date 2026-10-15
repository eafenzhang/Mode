/**
 * `rd/*` IPC 面注册（main 侧；命名与形状冻结见 ipcChannels.ts，renderer 桥由 T5 接）。
 *
 * 覆盖交付 C 的全部 invoke 与 push：
 * - 主控：get-peer-capability / start-session / send-signal / end-session（controllerSession）
 * - 被控设置面：get-server-state / set-server-enabled / revoke-client（经 host 读写被控存储）
 * - push：`rd/session-event`（主控会话 state/signal）
 *
 * 主控会话只有一条（单主控单被控）；被控会话与设置面彼此独立，本模块不做互斥（被控侧由 host 判 busy）。
 */
import { app, clipboard, ipcMain } from "electron";
import type { Locale } from "@mode/shared";
import {
  remoteDesktopCodecSchema,
  remoteDesktopQualitySchema,
  remoteDesktopSignalMessageSchema,
  type RemoteDesktopErrorCode,
} from "@mode/shared/remote-desktop";
import { REMOTE_DESKTOP_IPC } from "./ipcChannels.js";
import { requestRemoteDesktopPeerProbe } from "./hostBridge.js";
import {
  authorizeRemoteDesktopClient,
  readRemoteDesktopServerState,
  revokeRemoteDesktopClient,
  setRemoteDesktopServerEnabled,
} from "./settings.js";
import {
  handleRemoteDesktopAppQuit,
  handleRemoteDesktopServerDisabled,
  initializeRemoteDesktopSessionCoordinator,
  getRemoteDesktopServerSessionState,
} from "./coordinator.js";
import { createRemoteDesktopControllerSession } from "./controllerSession.js";
import { isRemoteDesktopSessionBusy } from "./sessionState.js";

export interface RemoteDesktopIpcOptions {
  logger: {
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
  };
  getLocale: () => Locale;
}

function readServerId(raw: unknown): string {
  return typeof raw === "string" ? raw.trim() : "";
}

export function registerRemoteDesktopIpcHandlers(options: RemoteDesktopIpcOptions): void {
  // 被控会话协调器（同意/指示/采集/信令/注入）与 hostBridge 绑定。
  initializeRemoteDesktopSessionCoordinator({
    logger: options.logger,
    getLocale: options.getLocale,
  });
  const controller = createRemoteDesktopControllerSession({ logger: options.logger });

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetPeerCapability, async (_event, rawServerId: unknown) => {
    const serverId = readServerId(rawServerId);
    if (!serverId) {
      return { remoteDesktop: false, remoteDesktopNative: false, online: false };
    }
    try {
      const info = await requestRemoteDesktopPeerProbe(serverId);
      return {
        remoteDesktop: info.remoteDesktop,
        // 版本门（纯直替）：透传探测结果，renderer 设备页据此判定可连接性。
        remoteDesktopNative: info.remoteDesktopNative,
        online: info.online,
        ...(info.name ? { name: info.name } : {}),
      };
    } catch (error) {
      // 宿主不可达/超时：一律按离线展示（能力位不猜测），由用户手动重试。
      options.logger.warn("[remote-desktop] peer capability probe failed:", error);
      return { remoteDesktop: false, remoteDesktopNative: false, online: false };
    }
  });

  ipcMain.handle(
    REMOTE_DESKTOP_IPC.StartSession,
    async (
      event,
      rawServerId: unknown,
      rawQuality?: unknown,
      rawCodec?: unknown,
    ): Promise<{ ok: true; mediaUrl?: string } | { error: RemoteDesktopErrorCode }> => {
      const serverId = readServerId(rawServerId);
      if (!serverId) {
        return { error: "internal" };
      }
      // 画质档位（P1-3）：非法/缺省按 undefined 走被控默认档（画质是优化参数，
      // 不值得让会话失败）。
      const parsedQuality = remoteDesktopQualitySchema.safeParse(rawQuality);
      const quality = parsedQuality.success ? parsedQuality.data : undefined;
      // 编码 codec 偏好（HEVC 选型）：同款宽容口径——非法/缺省按 undefined 走 h264。
      const parsedCodec = remoteDesktopCodecSchema.safeParse(rawCodec);
      const codec = parsedCodec.success ? parsedCodec.data : undefined;
      // 产品规则 9（被控中不可发起）：权威闸门在 main——本机被控状态机非 idle（含同意弹窗中）
      // 时拒绝，防双向互控的输入串台与画面套娃；设备页的置灰只是同规则的 UI 投影。
      if (isRemoteDesktopSessionBusy(getRemoteDesktopServerSessionState())) {
        return { error: "locally_controlled" };
      }
      return await controller.start(serverId, quality, codec, event.sender);
    },
  );

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetControlledState, async () => ({
    controlled: isRemoteDesktopSessionBusy(getRemoteDesktopServerSessionState()),
  }));

  ipcMain.handle(REMOTE_DESKTOP_IPC.SendSignal, (_event, rawMessage: unknown) => {
    const parsed = remoteDesktopSignalMessageSchema.safeParse(rawMessage);
    if (!parsed.success) {
      options.logger.warn("[remote-desktop] rd/send-signal payload rejected");
      return;
    }
    controller.sendSignal(parsed.data);
  });

  // 读本机剪贴板文本（P1.1b 剪贴板同步的观测面）：主进程 Electron clipboard，
  // 不经 renderer 的 navigator.clipboard（读剪贴板受 clipboard-read 权限约束）。
  ipcMain.handle(REMOTE_DESKTOP_IPC.ReadClipboardText, () => clipboard.readText());

  ipcMain.handle(REMOTE_DESKTOP_IPC.EndSession, async (_event, rawReason: unknown) => {
    await controller.end(rawReason === "error" ? "error" : "user-end");
  });

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetServerState, async () => {
    try {
      return await readRemoteDesktopServerState();
    } catch (error) {
      // 读失败按「关 + 空授权」fail-closed 展示；写路径仍以 host 现读为准。
      options.logger.warn("[remote-desktop] read server state failed:", error);
      return { enabled: false, authorizedClients: [] };
    }
  });

  ipcMain.handle(REMOTE_DESKTOP_IPC.SetServerEnabled, async (_event, rawEnabled: unknown) => {
    const enabled = rawEnabled === true;
    const state = await setRemoteDesktopServerEnabled(enabled);
    if (!enabled) {
      // 关开关 = 终止进行中会话（spec 事件顺序 ⑥），主控侧会收到 closed。
      handleRemoteDesktopServerDisabled();
    }
    return state;
  });

  ipcMain.handle(REMOTE_DESKTOP_IPC.RevokeClient, async (_event, rawClientId: unknown) => {
    const clientId = readServerId(rawClientId);
    if (!clientId) {
      throw new Error("clientId 不能为空");
    }
    return await revokeRemoteDesktopClient(clientId);
  });

  ipcMain.handle(REMOTE_DESKTOP_IPC.AuthorizeClient, async (_event, rawClientId: unknown) => {
    const clientId = readServerId(rawClientId);
    if (!clientId) {
      throw new Error("clientId 不能为空");
    }
    return await authorizeRemoteDesktopClient(clientId);
  });

  app.on("before-quit", () => {
    // 应用退出：被控先收口（bye 尽可能带出），主控再发 bye 关 WS（对端也会因 WS 断开而结束）。
    handleRemoteDesktopAppQuit();
    void controller.end("user-end");
    controller.dispose();
  });
}
