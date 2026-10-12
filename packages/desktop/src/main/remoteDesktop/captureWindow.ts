/**
 * 被控端隐藏采集窗（docs/specs/lan-remote-desktop.md 事件顺序 ③⑤；本机 spike 已验证
 * `show:false` + `backgroundThrottling:false` 可持续采集推流）。
 *
 * 职责：
 * - 创建隐藏 BrowserWindow（独立 partition 的 session），注册 `setDisplayMediaRequestHandler`
 *   按本窗口的 webContents 返回 desktopCapturer 的**屏幕源**（先例：electronBrowserWebmRecorder，
 *   但目标是屏幕源而不是 WebFrameMain）；
 * - 显示器枚举：已抽出到 `monitors.ts`（P4 删除采集窗后保留），此处 re-export 兼容既有 import；
 * - main ↔ 页面桥：信令下行 `deliver`、出站轮询 `takeOutbound`（页面无 preload，桥法见 capturePage.ts）；
 * - 从页面收回 DataChannel 输入：shared schema 严格校验 → 坐标换算（inputConversion）→ 批量
 *   （pointer 0ms 同 tick 聚合、control 即时，spec 质量与延迟参数）→ `onInput` 交给 coordinator
 *   下发 host 注入。
 *
 * 已知限制（一期不改）：多显示器混合 DPI 下 DIP 布局与物理布局非线性，坐标换算按
 * 「单显示器 / 统一缩放」假设；混合 DPI 的精确映射留待两机清单校准。
 */
import { randomUUID } from "node:crypto";
import { BrowserWindow, desktopCapturer, screen, session as electronSession } from "electron";
import type { BrowserWindowConstructorOptions, DesktopCapturerSource, Display, Session } from "electron";
import { remoteDesktopControlEventSchema, remoteDesktopPointerEventSchema, remoteDesktopSignalMessageSchema, type RemoteDesktopSignalMessage } from "@mode/shared/remote-desktop";
import {
  createRemoteDesktopInjectBatcher,
  convertControlEvent,
  convertPointerEvent,
  type RemoteDesktopPxRect,
} from "./inputConversion.js";
import type { RemoteDesktopRawInputEvent } from "./hostMessages.js";
import { writeRemoteDesktopCapturePage } from "./capturePage.js";
import { displayIdOf, physicalRectOf, unionRectOf } from "./monitors.js";

// 显示器枚举已抽出到 monitors.ts（P4 删除采集窗后保留）；此处 re-export 保持既有 import 不断。
export { displayIdOf, physicalRectOf, unionRectOf, buildRemoteDesktopMonitors, listRemoteDesktopMonitors } from "./monitors.js";

/** 页面可自行产生的信令类型（其余一律丢弃：页面无权伪造 monitors/hello 等主控向消息）。 */
const PAGE_EMITTABLE_SIGNAL_TYPES = new Set(["offer", "ice", "bye", "error"]);

export interface RemoteDesktopCaptureSession {
  /** main → 页面：对端信令（coordinator 已按会话规则筛过）。 */
  deliverSignal(message: RemoteDesktopSignalMessage): void;
  /** 选中显示器（会话内切换同样走它）；返回 false = 该 displayId 不在本机枚举内。 */
  selectMonitor(displayId: string): boolean;
  /** 结束收口：清批次、摘 handler、销毁窗口与临时文件。 */
  close(): Promise<void>;
}

export interface RemoteDesktopCaptureHandlers {
  /** 页面产生的出站信令（offer/ice/bye/error）→ coordinator → host → 对端。 */
  onOutboundSignal(message: RemoteDesktopSignalMessage): void;
  /** 已换算、已批量的注入帧 → coordinator → host。 */
  onInput(events: RemoteDesktopRawInputEvent[]): void;
  /** 页面致命错误（getUserMedia/PC 失败等）：会话按 error 收口。 */
  onFatalError(message: string): void;
  /** 非致命诊断。 */
  onDiagnostic(message: string): void;
}

export interface RemoteDesktopCaptureWindowOptions {
  handlers: RemoteDesktopCaptureHandlers;
  logger: { info(...args: unknown[]): void; warn(...args: unknown[]): void };
  /** 测试注入点。 */
  createWindow?: (windowOptions: BrowserWindowConstructorOptions) => BrowserWindow;
  captureSources?: () => Promise<DesktopCapturerSource[]>;
  displays?: () => Display[];
}

export async function openRemoteDesktopCaptureWindow(
  options: RemoteDesktopCaptureWindowOptions,
): Promise<RemoteDesktopCaptureSession> {
  const { handlers, logger } = options;
  const createWindow = options.createWindow ?? ((windowOptions) => new BrowserWindow(windowOptions));
  const captureSession: Session = electronSession.fromPartition(`mode-remote-desktop-${randomUUID()}`);
  const page = await writeRemoteDesktopCapturePage();
  let closed = false;
  let selectedDisplayId: string | null = null;
  let revealedSources: DesktopCapturerSource[] = [];
  const batcher = createRemoteDesktopInjectBatcher({
    now: () => Date.now(),
    schedule: (callback, delayMs) => setTimeout(callback, delayMs),
    cancel: (timer) => clearTimeout(timer),
    onFrame: (events) => handlers.onInput(events),
  });

  const win = createWindow({
    show: false,
    width: 640,
    height: 400,
    title: "Mode Remote Desktop Capture",
    webPreferences: {
      session: captureSession,
      backgroundThrottling: false,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
    },
  });

  const currentMonitorRect = (): { monitor: RemoteDesktopPxRect; virtual: RemoteDesktopPxRect } => {
    const displays = options.displays ? options.displays() : screen.getAllDisplays();
    const monitorDisplay = displays.find((display) => displayIdOf(display) === selectedDisplayId) ?? displays[0];
    const monitor =
      monitorDisplay === undefined
        ? { xPx: 0, yPx: 0, widthPx: 1, heightPx: 1 }
        : physicalRectOf(monitorDisplay);
    return { monitor, virtual: unionRectOf(displays) };
  };

  const handleOutboundItem = (item: unknown): void => {
    if (!item || typeof item !== "object") {
      return;
    }
    const kind = (item as { kind?: unknown }).kind;
    if (kind === "signal") {
      const parsed = remoteDesktopSignalMessageSchema.safeParse((item as { message?: unknown }).message);
      if (!parsed.success || !PAGE_EMITTABLE_SIGNAL_TYPES.has(parsed.data.type)) {
        logger.warn("[remote-desktop] dropping unexpected capture signal");
        return;
      }
      handlers.onOutboundSignal(parsed.data);
      return;
    }
    if (kind === "input") {
      const channel = (item as { channel?: unknown }).channel;
      const payload = (item as { payload?: unknown }).payload;
      if (channel === "pointer") {
        const parsed = remoteDesktopPointerEventSchema.safeParse(payload);
        if (!parsed.success) {
          return;
        }
        const { monitor, virtual } = currentMonitorRect();
        batcher.enqueuePointer(convertPointerEvent(parsed.data, monitor, virtual));
        return;
      }
      if (channel === "control") {
        const parsed = remoteDesktopControlEventSchema.safeParse(payload);
        if (!parsed.success) {
          return;
        }
        const { monitor, virtual } = currentMonitorRect();
        batcher.enqueueControl(convertControlEvent(parsed.data, monitor, virtual));
        return;
      }
      return;
    }
    if (kind === "diag") {
      handlers.onDiagnostic(String((item as { message?: unknown }).message ?? "").slice(0, 300));
    }
  };

  const deliver = (message: unknown): void => {
    if (closed || win.isDestroyed()) {
      return;
    }
    void win.webContents
      .executeJavaScript(`window.__rdCapture.deliver(${JSON.stringify(message)})`, true)
      .catch((error: unknown) => {
        if (!closed) {
          logger.warn("[remote-desktop] capture deliver failed:", error);
        }
      });
  };

  /** 出站轮询：同一时刻只有一个在飞的 executeJavaScript；空闲由页面侧 250ms 空数组兜底。 */
  const pumpOutbound = async (): Promise<void> => {
    while (!closed && !win.isDestroyed()) {
      let batch: unknown;
      try {
        batch = await win.webContents.executeJavaScript("window.__rdCapture.takeOutbound()", true);
      } catch {
        // 窗口/渲染器已销毁：轮询自然结束。
        return;
      }
      if (Array.isArray(batch)) {
        for (const item of batch) handleOutboundItem(item);
      }
    }
  };

  const close = async (): Promise<void> => {
    if (closed) {
      return;
    }
    closed = true;
    batcher.reset();
    try {
      captureSession.setDisplayMediaRequestHandler(null);
    } catch {
      // session 可能已随渲染进程销毁；handler 没有其它调用方。
    }
    try {
      if (!win.isDestroyed()) {
        win.destroy();
      }
    } catch {
      // 幂等失败忽略。
    }
    await page.cleanup();
  };

  captureSession.setDisplayMediaRequestHandler(
    async (request, callback) => {
      if (closed || request.frame !== win.webContents.mainFrame || !request.videoRequested || request.audioRequested) {
        callback({});
        return;
      }
      try {
        const sources = await (
          options.captureSources ??
          (() => desktopCapturer.getSources({ types: ["screen"], thumbnailSize: { width: 0, height: 0 } }))
        )();
        revealedSources = sources;
        // 选源顺序：被选中的显示器 → 主显示器 → 第一个屏幕源（选中显示器被拔掉时的兜底）。
        const primaryDisplayId = displayIdOf(screen.getPrimaryDisplay());
        const target =
          sources.find((source) => selectedDisplayId !== null && source.display_id === selectedDisplayId) ??
          sources.find((source) => source.display_id === primaryDisplayId) ??
          sources[0];
        if (!target) {
          callback({});
          return;
        }
        callback({ video: target });
      } catch (error) {
        logger.warn("[remote-desktop] display media handler failed:", error);
        callback({});
      }
    },
    { useSystemPicker: false },
  );

  await win.loadFile(page.path);
  await page.cleanup();

  win.webContents.on("render-process-gone", (_event, details) => {
    if (!closed) {
      handlers.onFatalError(`capture renderer gone: ${details.reason}`);
    }
  });

  void pumpOutbound();

  return {
    deliverSignal(message) {
      deliver(message);
    },
    selectMonitor(displayId) {
      // 首帧选择时 sources 可能还没枚举过：以 screen 模块的显示器 id 为主，枚举结果作补充。
      const displays = options.displays ? options.displays() : screen.getAllDisplays();
      const known =
        displays.some((display) => displayIdOf(display) === displayId) ||
        revealedSources.some((source) => source.display_id === displayId);
      if (!known) {
        return false;
      }
      selectedDisplayId = displayId;
      deliver({ type: "select-monitor", displayId });
      return true;
    },
    close,
  };
}
