import type { IServiceAccessor } from "@mode/services";
import type { RemoteDesktopLocalDisplay } from "@mode/shared/remote-desktop";

/**
 * 被控端显示器同步（LR-3，D7）：OS 能力留在 Tauri 壳，renderer 读快照后推给 Host。
 *
 * Host 不直接调 Tauri API；`hello` 到达时它用最近一次快照回 `monitors`。
 * 分辨率/位置变化由窗口 resize/move 事件触发重推；显示器热插拔在 v1 里依赖下一次事件。
 */

interface TauriMonitorLike {
  name?: string;
  size?: { width: number; height: number };
  position?: { x: number; y: number };
  scaleFactor?: number;
}

interface TauriWindowApi {
  availableMonitors?: () => Promise<TauriMonitorLike[]>;
  getCurrentWindow?: () => {
    onResized?: (handler: () => void) => Promise<() => void>;
    onMoved?: (handler: () => void) => Promise<() => void>;
  };
}

function tauriWindowApi(): TauriWindowApi | undefined {
  if (typeof globalThis === "undefined") {
    return undefined;
  }
  if (!("__TAURI__" in globalThis) && !("__TAURI_INTERNALS__" in globalThis)) {
    return undefined;
  }
  return (globalThis as { __TAURI__?: { window?: TauriWindowApi } }).__TAURI__?.window;
}

/** Tauri Monitor → 协议快照。位置/尺寸按物理像素；`displayId` 用枚举顺序，会话内稳定。 */
export function mapTauriMonitors(monitors: TauriMonitorLike[]): RemoteDesktopLocalDisplay[] {
  const valid = monitors.filter(
    (monitor) =>
      monitor &&
      monitor.size &&
      Number.isFinite(monitor.size.width) &&
      Number.isFinite(monitor.size.height) &&
      monitor.size.width > 0 &&
      monitor.size.height > 0,
  );
  const displays = valid.map((monitor, index) => ({
    displayId: String(index),
    label: monitor.name?.trim() || `Display ${index + 1}`,
    xPx: Math.trunc(monitor.position?.x ?? 0),
    yPx: Math.trunc(monitor.position?.y ?? 0),
    widthPx: Math.max(1, Math.trunc(monitor.size!.width)),
    heightPx: Math.max(1, Math.trunc(monitor.size!.height)),
    scaleFactor:
      Number.isFinite(monitor.scaleFactor) && (monitor.scaleFactor ?? 0) > 0
        ? (monitor.scaleFactor as number)
        : 1,
    primary: false,
  }));
  // Tauri 不提供 primary 位：原点显示器视为主屏；没有原点时取第一个。
  const primaryIndex = displays.findIndex((display) => display.xPx === 0 && display.yPx === 0);
  if (displays.length > 0) {
    displays[primaryIndex >= 0 ? primaryIndex : 0]!.primary = true;
  }
  return displays;
}

/** 读一次显示器快照并推给 Host；非 Tauri / 无 remoteDesktopService 时静默跳过。 */
export async function syncLocalDisplaysToHost(services: IServiceAccessor): Promise<void> {
  const service = services.remoteDesktopService;
  const api = tauriWindowApi();
  if (!service || typeof api?.availableMonitors !== "function") {
    return;
  }
  const monitors = await api.availableMonitors();
  await service.setLocalDisplays({ displays: mapTauriMonitors(monitors) });
}

/**
 * 启动同步：立即推一次，并在窗口 resize/move 后去抖重推。
 * 返回 disposer；renderer 生命周期内通常只调用一次。
 */
export function startLocalDisplaySync(services: IServiceAccessor): () => void {
  const api = tauriWindowApi();
  if (!api) {
    return () => undefined;
  }
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const sync = (): void => {
    if (disposed) {
      return;
    }
    if (timer) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = null;
      void syncLocalDisplaysToHost(services).catch(() => undefined);
    }, 200);
  };
  void syncLocalDisplaysToHost(services).catch(() => undefined);
  const unlisteners: Array<() => void> = [];
  const current = api.getCurrentWindow?.();
  for (const subscribe of [current?.onResized, current?.onMoved]) {
    if (typeof subscribe !== "function") {
      continue;
    }
    void subscribe
      .call(current, sync)
      .then((unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        unlisteners.push(unlisten);
      })
      .catch(() => undefined);
  }
  return () => {
    disposed = true;
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    for (const unlisten of unlisteners.splice(0)) {
      unlisten();
    }
  };
}
