/**
 * 远程桌面会话面（主控侧）：顶部操作栏（设备名/分辨率/全屏/断开）+ 视频全宽 letterbox + 底部状态条（fps/RTT/分辨率）。
 *
 * 交互要点（docs/specs/lan-remote-desktop.md UI 规则与产品规则 6/7）：
 * - 视频按 `object-fit: contain` 适配，画面内隐藏本地光标（对端光标随帧显示，避免双光标）；
 * - 容器 `tabIndex=0` 自动聚焦，键盘输入直接落到对端；
 * - 结束态（对端挂断 / 错误码 / 媒体失败）显示原因并回到设备页；本端断开立即返回。
 */
import { useEffect, useState } from "react";
import {
  ArrowLeft,
  LoaderIcon,
  Maximize2,
  Minimize2,
  MonitorSmartphone,
  ScreenShare,
  Unplug,
} from "lucide-react";
import { Button } from "@/components/ui/button.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useRemoteDesktopSession } from "@/remote-desktop/useRemoteDesktopSession.js";
import type { RemoteDesktopSessionCloseKind } from "@/remote-desktop/sessionReducer.js";

/** 对端结束/失败后停留时长：先让用户看清原因，再自动回设备页。 */
const CLOSED_RETURN_DELAY_MS = 4000;

function resolveCloseReasonId(
  closeKind: RemoteDesktopSessionCloseKind | null,
  errorCode: string | null,
): string {
  if (closeKind === "user") {
    return "remoteDesktop.session.closed.user";
  }
  if (errorCode) {
    return `remoteDesktop.error.${errorCode}`;
  }
  if (closeKind === "remote") {
    return "remoteDesktop.session.closed.remote";
  }
  return "remoteDesktop.session.closed.error";
}

export function RemoteDesktopSession({
  serverId,
  deviceName,
  onExit,
}: {
  serverId: string;
  deviceName: string;
  onExit: () => void;
}) {
  const { intl } = useModeIntl();
  const { view, stats, peerConnectionState, containerRef, videoRef, selectMonitor, disconnect } =
    useRemoteDesktopSession({ serverId });
  const [isFullscreen, setIsFullscreen] = useState(false);

  // 工具条交互后把焦点还给视频面：否则键盘输入会留在按钮/下拉上，打不到对端。
  const refocusContainer = () => {
    containerRef.current?.focus({ preventScroll: true });
  };

  // 对端结束/失败：显示原因后自动回设备页；本端断开由按钮直接返回。
  useEffect(() => {
    if (view.phase !== "closed" || view.closeKind === "user") {
      return;
    }
    const timer = window.setTimeout(() => onExit(), CLOSED_RETURN_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [view.phase, view.closeKind, onExit]);

  useEffect(() => {
    const onChange = () => {
      setIsFullscreen(Boolean(document.fullscreenElement));
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const toggleFullscreen = () => {
    const element = containerRef.current;
    if (!element) {
      return;
    }
    if (document.fullscreenElement) {
      void document
        .exitFullscreen()
        .catch(() => undefined)
        .finally(refocusContainer);
      return;
    }
    void element
      .requestFullscreen()
      .catch(() => undefined)
      .finally(refocusContainer);
  };

  const selectedMonitor = view.monitors.find(
    (monitor) => monitor.displayId === view.selectedDisplayId,
  );
  const statusText =
    view.phase === "closed"
      ? resolveCloseReasonId(view.closeKind, view.errorCode)
      : // ICE 短暂断开会自动恢复：画面仍在但可能冻结，状态条先给出提示（不终止会话）。
        peerConnectionState === "disconnected"
        ? "remoteDesktop.session.reconnecting"
        : view.mediaStatus === "live"
          ? "remoteDesktop.session.live"
          : view.offerReceived
            ? "remoteDesktop.session.negotiating"
            : view.phase === "connected"
              ? "remoteDesktop.session.waitingPeer"
              : "remoteDesktop.session.connecting";

  return (
    <div
      ref={containerRef}
      tabIndex={0}
      data-testid="remote-desktop-session"
      className="relative flex h-full min-h-0 flex-col bg-background outline-none"
    >
      {/* 操作栏在顶部（spec UI 规则·会话面）：border-b；状态条留底部。 */}
      <div
        data-testid="remote-desktop-toolbar"
        className="flex flex-wrap items-center gap-2 border-b border-border bg-panel px-3 py-2"
      >
        <div className="flex min-w-0 items-center gap-2 text-ui-base text-foreground">
          <MonitorSmartphone className="size-4 shrink-0 text-foreground-subtle" />
          <span className="truncate">{deviceName}</span>
        </div>
        <div className="ms-auto flex flex-wrap items-center gap-2">
          {view.monitors.length > 0 ? (
            <Select
              value={view.selectedDisplayId ?? undefined}
              disabled={view.phase === "closed"}
              onValueChange={(displayId) => {
                selectMonitor(displayId);
                // 选完把焦点还给视频面（键盘输入继续直达对端）。
                refocusContainer();
              }}
            >
              <SelectTrigger
                size="sm"
                aria-label={intl.formatMessage({ id: "remoteDesktop.session.monitor" })}
                className="w-fit max-w-[16rem]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                {view.monitors.map((monitor) => (
                  <SelectItem key={monitor.displayId} value={monitor.displayId}>
                    {`${monitor.label || monitor.displayId} · ${monitor.widthPx}×${monitor.heightPx}`}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : null}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={toggleFullscreen}
            aria-label={intl.formatMessage({
              id: isFullscreen
                ? "remoteDesktop.session.exitFullscreen"
                : "remoteDesktop.session.fullscreen",
            })}
          >
            {isFullscreen ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
            {intl.formatMessage({
              id: isFullscreen
                ? "remoteDesktop.session.exitFullscreen"
                : "remoteDesktop.session.fullscreen",
            })}
          </Button>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            onClick={() => {
              disconnect();
              onExit();
            }}
          >
            <Unplug className="size-4" />
            {intl.formatMessage({ id: "remoteDesktop.session.disconnect" })}
          </Button>
        </div>
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {/* cursor-none：画面内只显示对端随帧光标（spec 产品规则 7）。 */}
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted
          className="h-full w-full cursor-none object-contain"
        />
        {view.mediaStatus !== "live" ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
            <div className="flex items-center gap-2 rounded-lg border border-border bg-popover/95 px-3 py-2 text-ui-base text-foreground shadow-sm">
              {view.phase === "closed" ? (
                <Unplug className="size-4 text-destructive" />
              ) : (
                <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
              )}
              <span>{intl.formatMessage({ id: statusText })}</span>
            </div>
          </div>
        ) : null}
        {view.phase === "closed" ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-background/85">
            <p className="max-w-md text-center text-ui-base text-foreground">
              {intl.formatMessage({ id: statusText })}
            </p>
            <Button type="button" variant="outline" size="sm" onClick={() => onExit()}>
              <ArrowLeft className="size-4" />
              {intl.formatMessage({ id: "remoteDesktop.session.backToDevices" })}
            </Button>
          </div>
        ) : null}
      </div>

      <div
        data-testid="remote-desktop-status"
        className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-panel px-3 py-1.5 text-ui-xs text-foreground-subtle"
      >
        <span className="flex items-center gap-1.5">
          <ScreenShare className="size-3.5 shrink-0" />
          {intl.formatMessage({ id: statusText })}
        </span>
        {stats.fps !== null ? (
          <span>
            {intl.formatMessage({ id: "remoteDesktop.session.fps" }, { value: String(stats.fps) })}
          </span>
        ) : null}
        {stats.rttMs !== null ? (
          <span>
            {intl.formatMessage(
              { id: "remoteDesktop.session.rtt" },
              { value: String(stats.rttMs) },
            )}
          </span>
        ) : null}
        {stats.width !== null && stats.height !== null ? (
          <span>{`${stats.width}×${stats.height}`}</span>
        ) : selectedMonitor ? (
          <span>{`${selectedMonitor.widthPx}×${selectedMonitor.heightPx}`}</span>
        ) : null}
      </div>
    </div>
  );
}
