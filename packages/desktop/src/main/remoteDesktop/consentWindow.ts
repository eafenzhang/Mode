/**
 * 被控端同意窗（docs/specs/lan-remote-desktop.md 产品规则 2、UI 规则「被控端」）：
 * 仅对**未记住**的设备弹出；显示发起设备名、「允许 / 拒绝」、「记住此设备」与 60s 倒计时。
 *
 * 窗口形态沿用 windowsCuaOperationIndicator 先例（frameless / transparent / alwaysOnTop /
 * skipTaskbar / 定位到当前鼠标显示器顶部居中 / 不抢焦点）。与它的差异只有一个：这里需要用户
 * 点击，因此**不** setIgnoreMouseEvents，并使用可聚焦窗口（点击时才获得焦点）。
 *
 * 用户动作经 `window.__rd.takeAction()` 长轮询收回（无 preload，桥法见 windowsContent.ts）。
 * 超时真相在 main：本模块自带 60s 计时器，页面倒计时只做显示（页面卡死也能收口）。
 */
import { BrowserWindow, screen } from "electron";
import type { BrowserWindowConstructorOptions } from "electron";
import type { Locale } from "@mode/shared";
import { consentWindowDataUrl, consentWindowSize } from "./windowsContent.js";

export const REMOTE_DESKTOP_CONSENT_TIMEOUT_MS = 60_000;
const SHADOW_TOP_INSET = 6;

export type RemoteDesktopConsentDecision =
  | { kind: "allow"; remember: boolean }
  | { kind: "deny" }
  | { kind: "timeout" }
  /** 窗口创建/加载失败，或动作收回失败：调用方按拒绝的等价路径收口。 */
  | { kind: "unavailable" };

export interface RemoteDesktopConsentWindow {
  waitForDecision(): Promise<RemoteDesktopConsentDecision>;
  close(): void;
}

function isDecision(value: unknown): value is { kind: string } {
  return Boolean(value) && typeof value === "object" && typeof (value as { kind?: unknown }).kind === "string";
}

export function openRemoteDesktopConsentWindow(options: {
  locale: Locale;
  deviceLabel: string;
  logger: { warn(...args: unknown[]): void };
  timeoutMs?: number;
  createWindow?: (windowOptions: BrowserWindowConstructorOptions) => BrowserWindow;
}): RemoteDesktopConsentWindow {
  const timeoutMs = options.timeoutMs ?? REMOTE_DESKTOP_CONSENT_TIMEOUT_MS;
  const createWindow = options.createWindow ?? ((windowOptions) => new BrowserWindow(windowOptions));
  const { width, height } = consentWindowSize(options.locale);
  let settled = false;
  let destroyed = false;
  let resolveDecision: ((decision: RemoteDesktopConsentDecision) => void) | null = null;
  const decision = new Promise<RemoteDesktopConsentDecision>((resolve) => {
    resolveDecision = resolve;
  });

  const settle = (value: RemoteDesktopConsentDecision): void => {
    if (settled) {
      return;
    }
    settled = true;
    if (timeoutTimer) {
      clearTimeout(timeoutTimer);
    }
    resolveDecision?.(value);
  };

  let win: BrowserWindow | null = null;
  let timeoutTimer: ReturnType<typeof setTimeout> | null = null;

  const close = (): void => {
    destroyed = true;
    if (timeoutTimer) {
      clearTimeout(timeoutTimer);
      timeoutTimer = null;
    }
    // 必须总是收敛 waitForDecision：程序化关闭（会话被收口/应用退出）与用户关窗一样，
    // 都等价拒绝；否则 coordinator 的 open-consent 动作会永久挂住，收口链停摆。
    settle({ kind: "deny" });
    try {
      if (win && !win.isDestroyed()) {
        win.destroy();
      }
    } catch {
      // 窗口已随进程退出时 destroy 允许幂等失败。
    }
    win = null;
  };

  try {
    win = createWindow({
      width,
      height,
      alwaysOnTop: true,
      frame: false,
      hasShadow: false,
      resizable: false,
      show: false,
      skipTaskbar: true,
      transparent: true,
      backgroundColor: "#00000000",
      autoHideMenuBar: true,
      fullscreenable: false,
      maximizable: false,
      minimizable: false,
      movable: false,
      webPreferences: {
        contextIsolation: true,
        devTools: false,
        nodeIntegration: false,
        sandbox: true,
      },
    });
    const point = screen.getCursorScreenPoint();
    const { workArea } = screen.getDisplayNearestPoint(point);
    win.setBounds({
      width,
      height,
      x: Math.round(workArea.x + (workArea.width - width) / 2),
      y: Math.round(workArea.y + 12 - SHADOW_TOP_INSET),
    });
    win.on("closed", () => {
      // 用户/系统关窗：等价拒绝（settle 幂等，程序化 close 已先 settle）。
      settle({ kind: "deny" });
    });
    const loaded = win.loadURL(
      consentWindowDataUrl(options.locale, options.deviceLabel, Math.round(timeoutMs / 1000)),
    );
    void loaded
      .then(async () => {
        if (destroyed || !win || win.isDestroyed()) {
          return;
        }
        win.showInactive();
        win.setAlwaysOnTop(true, "screen-saver");
        win.moveTop();
        const action: unknown = await win.webContents.executeJavaScript(
          "window.__rd.takeAction()",
          true,
        );
        if (!isDecision(action)) {
          settle({ kind: "deny" });
          return;
        }
        if (action.kind === "allow") {
          settle({
            kind: "allow",
            remember: (action as { remember?: unknown }).remember === true,
          });
          return;
        }
        settle({ kind: action.kind === "timeout" ? "timeout" : "deny" });
      })
      .catch((error: unknown) => {
        options.logger.warn("[remote-desktop] consent window failed:", error);
        settle({ kind: "unavailable" });
      });
    timeoutTimer = setTimeout(() => {
      // 页面倒计时是显示层；超时的权威计时器在这里（页面卡死也能收口为拒绝）。
      settle({ kind: "timeout" });
      close();
    }, timeoutMs);
  } catch (error) {
    options.logger.warn("[remote-desktop] consent window create failed:", error);
    settle({ kind: "unavailable" });
  }

  return {
    waitForDecision: () => decision,
    close,
  };
}
