/**
 * `rd/*` IPC 面注册（main 侧；通道名与形状见 ipcChannels.ts）。
 *
 * 本模块是主控侧唯一的侧效应入口：发起 = 生成临时 `.rdp` + 启动 mstsc；
 * 被控设置面 = 提示性共享开关 + 系统 RDP 状态（经 host 现读现写）。
 * 画面/输入/剪贴板/音频没有任何通道——全部由 Windows RDP 承担
 * （docs/specs/remote-desktop-system-rdp.md 架构节）。
 */
import { spawn } from "node:child_process";
import { join } from "node:path";
import { app, ipcMain } from "electron";
import {
  rdpConnectionOptionsSchema,
  rdpConnectRequestSchema,
  type RdpConnectResult,
  type RdpConnectionOptions,
  type RdpServerState,
} from "@mode/shared/remote-desktop-rdp";
import { REMOTE_DESKTOP_IPC } from "./ipcChannels.js";
import {
  requestRemoteDesktopPeerProbe,
  requestRemoteDesktopServerState,
  requestRemoteDesktopSetServerEnabled,
} from "./hostBridge.js";
import { createRdpLauncher } from "./rdpLauncher.js";
import { createRdpOptionsStore } from "./rdpOptions.js";

export interface RemoteDesktopIpcOptions {
  logger: {
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
  };
}

/**
 * 终止进程树：mstsc 会派生窗口/子进程，只杀父进程可能留下孤儿窗口。
 * 进程已消失时 taskkill 返回非 0，属于正常竞态（exit 处理已清理跟踪）。
 */
async function killProcessTree(pid: number): Promise<void> {
  await new Promise<void>((resolve) => {
    const killer = spawn("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      windowsHide: true,
      stdio: "ignore",
    });
    killer.once("exit", () => resolve());
    killer.once("error", () => resolve());
  });
}

function readNonEmptyString(raw: unknown): string {
  return typeof raw === "string" ? raw.trim() : "";
}

export function registerRemoteDesktopIpcHandlers(options: RemoteDesktopIpcOptions): void {
  const launcher = createRdpLauncher({
    // detached：会话不随应用退出而终止（mstsc 是独立进程，spec 事件顺序 ④）。
    spawn: (command, args) =>
      spawn(command, [...args], { detached: true, stdio: "ignore", windowsHide: false }),
    tempDir: join(app.getPath("temp"), "mode-rdp"),
    killTree: killProcessTree,
    logger: options.logger,
  });
  const optionsStore = createRdpOptionsStore(app.getPath("userData"));

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetPeerCapability, async (_event, rawServerId: unknown) => {
    const serverId = readNonEmptyString(rawServerId);
    if (!serverId) {
      return { remoteDesktop: false, online: false };
    }
    try {
      const info = await requestRemoteDesktopPeerProbe(serverId);
      return {
        remoteDesktop: info.remoteDesktop,
        ...(info.systemRdp ? { systemRdp: info.systemRdp } : {}),
        online: info.online,
        ...(info.name ? { name: info.name } : {}),
      };
    } catch (error) {
      // 宿主不可达/超时：一律按离线展示（能力位不猜测），由用户手动重试。
      options.logger.warn("[remote-desktop] peer capability probe failed:", error);
      return { remoteDesktop: false, online: false };
    }
  });

  ipcMain.handle(
    REMOTE_DESKTOP_IPC.Connect,
    async (_event, rawRequest: unknown): Promise<RdpConnectResult> => {
      const parsed = rdpConnectRequestSchema.safeParse(rawRequest);
      if (!parsed.success) {
        // 非法载荷按地址问题归因：renderer 只可能从设备记录/用户输入拼出这两个值。
        options.logger.warn("[remote-desktop] rd/connect payload rejected");
        return { ok: false, reason: "invalid_address" };
      }
      return await launcher.connect(parsed.data);
    },
  );

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetSessions, () => launcher.list());

  ipcMain.handle(REMOTE_DESKTOP_IPC.Disconnect, async (_event, rawSessionId: unknown) => {
    const sessionId = readNonEmptyString(rawSessionId);
    if (!sessionId) {
      return { ok: false };
    }
    return await launcher.disconnect(sessionId);
  });

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetRdpOptions, async (_event, rawServerId: unknown) => {
    const serverId = readNonEmptyString(rawServerId);
    return serverId ? await optionsStore.read(serverId) : {};
  });

  ipcMain.handle(
    REMOTE_DESKTOP_IPC.SetRdpOptions,
    async (_event, rawServerId: unknown, rawOptions: unknown): Promise<RdpConnectionOptions> => {
      const serverId = readNonEmptyString(rawServerId);
      if (!serverId) {
        return {};
      }
      const parsed = rdpConnectionOptionsSchema.safeParse(rawOptions);
      if (!parsed.success) {
        options.logger.warn("[remote-desktop] rd/set-rdp-options payload rejected");
        return await optionsStore.read(serverId);
      }
      await optionsStore.write(serverId, parsed.data);
      return await optionsStore.read(serverId);
    },
  );

  ipcMain.handle(REMOTE_DESKTOP_IPC.GetServerState, async (): Promise<RdpServerState> => {
    try {
      return await requestRemoteDesktopServerState();
    } catch (error) {
      // 读失败按「未共享 + 状态未知」fail-closed 展示；写路径仍以 host 现读为准。
      options.logger.warn("[remote-desktop] read server state failed:", error);
      return { sharingEnabled: false };
    }
  });

  ipcMain.handle(
    REMOTE_DESKTOP_IPC.SetServerEnabled,
    async (_event, rawEnabled: unknown): Promise<RdpServerState> => {
      // 关共享开关**不终止任何 RDP 连接**：它拦不住系统 RDP（spec 产品规则 5），
      // 只是让本机不再出现在对端设备页。
      return await requestRemoteDesktopSetServerEnabled(rawEnabled === true);
    },
  );

  app.on("before-quit", () => {
    // 只清理临时 `.rdp`；不终止会话——用户关应用不该掐断正在用的远程桌面。
    void launcher.cleanupOnQuit();
  });
}
