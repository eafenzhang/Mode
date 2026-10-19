import { randomBytes } from "node:crypto";

import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  type RemoteDesktopLocalDisplay,
  type RemoteDesktopMonitor,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";

import {
  RemoteDesktopHostMessageTypes,
  type RemoteDesktopMessageToHost,
} from "./remoteDesktopMessages.js";

/**
 * 被控侧协调器（LR-3，Host 版）：把主控信令翻译成 Host 的媒体命令。
 *
 * Electron 线的 `main/remoteDesktop/coordinator.ts` 还负责同意窗、指示条与系统剪贴板；
 * 这里只保留纯协调部分：
 * - `hello` → 用 renderer 推来的显示器快照回 `monitors`；
 * - `select-monitor` → 首次发 `media-open` + `MediaStart`，换屏发 `MediaSwitch`；
 * - `MediaStatus` → `freeze` / `unfreeze`（媒体起不来时回 `error{unsupported}`）；
 * - 会话关闭 → `MediaStop` 并清会话材料。
 *
 * 媒体密钥/盐只在 Host 内存与 helper 之间使用，不落盘、不进日志。
 */

export interface ControlledCoordinatorLogger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

export interface RemoteDesktopMediaStatusMessage {
  frozen: boolean;
  reason?: string;
  unavailable?: string;
}

export interface RemoteDesktopControlledCoordinator {
  reset(): void;
  /** 主控信令（`SignalFromHost`）。 */
  handleControllerSignal(message: RemoteDesktopSignalMessage): void;
  /** helper 采集状态（`MediaStatus`）。 */
  handleMediaStatus(message: RemoteDesktopMediaStatusMessage): void;
  /** 会话关闭（`SessionClosed`）：停采集并清材料。 */
  handleSessionClosed(): void;
}

export interface CreateRemoteDesktopControlledCoordinatorOptions {
  getDisplays: () => RemoteDesktopLocalDisplay[];
  /** 写回主控信令（`SignalToHost`）。 */
  sendSignal: (message: RemoteDesktopSignalMessage) => void;
  /** 发给被控 server 的媒体命令（MediaStart/Switch/Stop）。 */
  sendHostMessage: (message: RemoteDesktopMessageToHost) => void;
  logger: ControlledCoordinatorLogger;
  /** 测试注入：媒体密钥/盐（64 hex / 16 hex）。 */
  createKeySalt?: () => { key: string; salt: string };
}

function toMonitor(display: RemoteDesktopLocalDisplay): RemoteDesktopMonitor {
  return {
    displayId: display.displayId,
    label: display.label,
    widthPx: display.widthPx,
    heightPx: display.heightPx,
    scaleFactor: display.scaleFactor,
    primary: display.primary,
  };
}

function toRect(display: RemoteDesktopLocalDisplay): {
  x: number;
  y: number;
  width: number;
  height: number;
} {
  return {
    x: display.xPx,
    y: display.yPx,
    width: Math.max(1, display.widthPx),
    height: Math.max(1, display.heightPx),
  };
}

function unionRect(displays: RemoteDesktopLocalDisplay[]): {
  x: number;
  y: number;
  width: number;
  height: number;
} {
  if (displays.length === 0) {
    return { x: 0, y: 0, width: 1, height: 1 };
  }
  const left = Math.min(...displays.map((display) => display.xPx));
  const top = Math.min(...displays.map((display) => display.yPx));
  const right = Math.max(...displays.map((display) => display.xPx + Math.max(1, display.widthPx)));
  const bottom = Math.max(
    ...displays.map((display) => display.yPx + Math.max(1, display.heightPx)),
  );
  return { x: left, y: top, width: Math.max(1, right - left), height: Math.max(1, bottom - top) };
}

export function createRemoteDesktopControlledCoordinator(
  options: CreateRemoteDesktopControlledCoordinatorOptions,
): RemoteDesktopControlledCoordinator {
  const createKeySalt =
    options.createKeySalt ??
    (() => ({
      key: randomBytes(32).toString("hex"),
      salt: randomBytes(8).toString("hex"),
    }));
  let mediaOpen = false;

  const sendError = (code: "unsupported" | "internal", message: string): void => {
    options.sendSignal({ type: "error", code, message });
  };

  return {
    reset(): void {
      mediaOpen = false;
    },

    handleControllerSignal(message): void {
      switch (message.type) {
        case "hello": {
          if (message.protocolVersion !== REMOTE_DESKTOP_PROTOCOL_VERSION) {
            sendError(
              "unsupported",
              `protocol version ${message.protocolVersion} is not supported`,
            );
            return;
          }
          const displays = options.getDisplays();
          if (displays.length === 0) {
            options.logger.warn("[remote-desktop] no local displays reported by renderer");
            sendError("unsupported", "local displays unavailable");
            return;
          }
          options.sendSignal({ type: "monitors", monitors: displays.map(toMonitor) });
          return;
        }
        case "select-monitor": {
          const displays = options.getDisplays();
          const display = displays.find((candidate) => candidate.displayId === message.displayId);
          if (!display) {
            options.logger.warn(
              `[remote-desktop] select-monitor unknown displayId=${message.displayId}`,
            );
            sendError("internal", "unknown monitor");
            return;
          }
          const rect = toRect(display);
          const virtualRect = unionRect(displays);
          if (!mediaOpen) {
            const { key, salt } = createKeySalt();
            // 先下发 media-open（此时信令 WS 已 attach），再让 host 起采集。
            options.sendSignal({ type: "media-open", key, salt });
            options.sendHostMessage({
              type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
              rect,
              virtualRect,
              key,
              salt,
            });
            mediaOpen = true;
            options.logger.info(
              `[remote-desktop] media start requested, displayId=${message.displayId}`,
            );
            return;
          }
          options.sendHostMessage({
            type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
            rect,
            virtualRect,
          });
          options.logger.info(
            `[remote-desktop] media switch requested, displayId=${message.displayId}`,
          );
          return;
        }
        case "bye":
          // 被控 server 会在转发 bye 后关闭 WS；媒体收口由 SessionClosed 统一处理。
          return;
        case "error":
          // 主控端错误：转发已在 server 层完成，等待 WS 关闭后的 SessionClosed。
          return;
        default:
          // 被控端不应收到 monitors / media-open / freeze / unfreeze（方向相反）。
          options.logger.warn(`[remote-desktop] dropping unexpected signal ${message.type}`);
      }
    },

    handleMediaStatus(message): void {
      if (message.unavailable) {
        // 采集从未建立：主控必须收到稳定错误码，而不是对着零帧干等。
        sendError("unsupported", message.unavailable);
        return;
      }
      if (message.frozen) {
        const reason = message.reason === "locked" ? "locked" : "access-lost";
        options.sendSignal({ type: "freeze", reason });
        return;
      }
      options.sendSignal({ type: "unfreeze" });
    },

    handleSessionClosed(): void {
      options.sendHostMessage({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop,
      });
      mediaOpen = false;
    },
  };
}
