/**
 * 被控端会话指示浮层（docs/specs/lan-remote-desktop.md 产品规则 3）：
 * 会话期间常驻「<设备名> 正在远程控制本机」+「结束」按钮；点「结束」= 本地立即终止。
 *
 * 窗口形态沿用 windowsCuaOperationIndicator 先例（frameless / transparent / alwaysOnTop /
 * skipTaskbar / 定位到当前鼠标显示器顶部居中 / 不抢焦点）；差异：这里必须可点击，
 * 因此不 setIgnoreMouseEvents。「结束」动作经 `window.__rd.takeAction()` 长轮询收回。
 */
import { BrowserWindow, screen } from "electron";
import type { BrowserWindowConstructorOptions } from "electron";
import type { Locale } from "@mode/shared";
import { indicatorWindowDataUrl, indicatorWindowSize } from "./windowsContent.js";

const SHADOW_TOP_INSET = 6;

export interface RemoteDesktopIndicatorWindow {
  /** 会话结束收口：撤销指示（销毁窗口）。 */
  close(): void;
}

export function openRemoteDesktopIndicatorWindow(options: {
  locale: Locale;
  deviceLabel: string;
  /** 用户点「结束」：由 coordinator 走 closeSession("local-end") 单出口。 */
  onEndRequested: () => void;
  logger: { warn(...args: unknown[]): void };
  createWindow?: (windowOptions: BrowserWindowConstructorOptions) => BrowserWindow;
}): RemoteDesktopIndicatorWindow {
  const createWindow = options.createWindow ?? ((windowOptions) => new BrowserWindow(windowOptions));
  const { width, height } = indicatorWindowSize(options.locale);
  let win: BrowserWindow | null = null;
  let closed = false;

  const close = (): void => {
    if (closed) {
      return;
    }
    closed = true;
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
      win = null;
      if (!closed) {
        // 系统意外关闭浮层不等于会话结束（会话仍由 WS/采集窗收口），只标记窗口不可用。
        closed = true;
      }
    });
    void win
      .loadURL(indicatorWindowDataUrl(options.locale, options.deviceLabel))
      .then(async () => {
        if (closed || !win || win.isDestroyed()) {
          return;
        }
        win.showInactive();
        win.setAlwaysOnTop(true, "screen-saver");
        win.moveTop();
        const action: unknown = await win.webContents.executeJavaScript(
          "window.__rd.takeAction()",
          true,
        );
        if (
          action &&
          typeof action === "object" &&
          (action as { kind?: unknown }).kind === "end"
        ) {
          options.onEndRequested();
        }
      })
      .catch((error: unknown) => {
        options.logger.warn("[remote-desktop] indicator window failed:", error);
      });
  } catch (error) {
    options.logger.warn("[remote-desktop] indicator window create failed:", error);
  }

  return { close };
}
