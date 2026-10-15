/**
 * 远程桌面会话面（主控侧）：顶部操作栏（设备名/画质/显示器/全屏/断开）+ 画面全宽 letterbox + 底部状态条（fps/RTT/分辨率）。
 *
 * 交互要点（docs/specs/lan-remote-desktop-native-media.md；产品规则 6/7 沿用主 spec）：
 * - 画面（原生媒体面为 canvas，解码即画）按 `object-fit: contain` 适配，画面内隐藏本地光标
 *   （对端光标已合成进帧，避免双光标）；
 * - 容器 `tabIndex=0` 自动聚焦，键盘输入直接落到对端；
 * - 结束态（对端挂断 / 错误码 / 媒体失败）显示原因并回到设备页；本端断开立即返回；
 * - 画质档位在操作栏选择（P1-3 入口 2026-10-13 由设备卡迁入）：会话中改档 = bye +
 *   P2-1 重连以新档重建，期间以「正在恢复」遮蔽本地关闭瞬态（不出关闭层、不触发回页）。
 */
import { useEffect, useState } from "react";
import {
  ArrowLeft,
  ClipboardCheck,
  ClipboardX,
  LoaderIcon,
  Lock,
  Maximize2,
  Minimize2,
  MonitorSmartphone,
  ScreenShare,
  Unplug,
} from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { cn } from "@/components/lib/utils.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import type { RemoteDesktopQuality } from "@mode/shared/remote-desktop";
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
  quality,
  onQualityChange,
  topOverlayClearancePaddingClass,
  onExit,
}: {
  serverId: string;
  deviceName: string;
  /** 画质档位（P1-3）：页级记忆（本页状态），发起会话与改档重建都以它下发。 */
  quality: RemoteDesktopQuality;
  /** 档位选择回写页级记忆（会话面只读展示当前档）。 */
  onQualityChange: (quality: RemoteDesktopQuality) => void;
  /** 左侧栏收起时让出顶部浮层左上入口宽度的 ps-* 类（chat 页 WorkspaceHeader 同款规则）。 */
  topOverlayClearancePaddingClass?: string | null;
  onExit: () => void;
}) {
  const { intl } = useModeIntl();
  const {
    view,
    stats,
    containerRef,
    canvasRef,
    selectMonitor,
    disconnect,
    switchQuality,
    clipboardEnabled,
    setClipboardEnabled,
  } =
    // 页级记忆回写走 hook 的 onQualityChanged（手动改档与自动降档共用）。
    useRemoteDesktopSession({ serverId, quality, onQualityChanged: onQualityChange });
  const [isFullscreen, setIsFullscreen] = useState(false);

  // 工具条交互后把焦点还给画面：否则键盘输入会留在按钮/下拉上，打不到对端。
  const refocusContainer = () => {
    containerRef.current?.focus({ preventScroll: true });
  };

  // 对端结束/失败：显示原因后自动回设备页；本端断开由按钮直接返回。
  // 改档重建的瞬态（closed && reconnecting）不算结束：不启回页计时，由「正在恢复」覆盖。
  useEffect(() => {
    if (
      view.phase !== "closed" ||
      view.closeKind === "user" ||
      view.mediaStatus === "reconnecting"
    ) {
      return;
    }
    const timer = window.setTimeout(() => onExit(), CLOSED_RETURN_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [view.phase, view.closeKind, view.mediaStatus, onExit]);

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
    // 优先级：冻结（对端已锁屏，画面不再更新，优先提示）> 重连/改档重建（P2-1 与
    // 改档瞬态——此时 phase 可能已是 closed，但「正在恢复」才是真实状态）> 结束原因
    // > 首帧缺失（已连接但窗口内零帧，spec「媒体面可观测性与首帧约束」2026-10-14）
    // > 媒体/协商/连接进度。
    view.frozen
      ? "remoteDesktop.session.frozen"
      : view.mediaStatus === "reconnecting"
        ? "remoteDesktop.session.reconnecting"
        : view.phase === "closed"
          ? resolveCloseReasonId(view.closeKind, view.errorCode)
          : view.awaitingFirstFrame
            ? "remoteDesktop.session.awaitingFrames"
            : view.mediaStatus === "live"
              ? "remoteDesktop.session.live"
              : view.mediaOpenReceived
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
      {/* 操作栏在顶部（spec UI 规则·会话面）：border-b；状态条留底部。
          左侧栏收起时 DesktopTopOverlay 的左上入口悬在操作栏左上，设备名按 chat 页
          WorkspaceHeader 同款 ps-* 规则让位，否则两者重叠。 */}
      <div
        data-testid="remote-desktop-toolbar"
        className={cn(
          "flex flex-wrap items-center gap-2 border-b border-border bg-panel pe-3 py-2",
          topOverlayClearancePaddingClass ?? "ps-3",
        )}
      >
        <div className="flex min-w-0 items-center gap-2 text-ui-base text-foreground">
          <MonitorSmartphone className="size-4 shrink-0 text-foreground-subtle" />
          <span className="truncate">{deviceName}</span>
        </div>
        <div className="ms-auto flex flex-wrap items-center gap-2">
          {/* 剪贴板同步开关（P1.1）：本端立即生效并上行给被控中继；关闭只停同步，
              不影响画面与输入。未连接时禁用（会话建立后才存在同步面）。 */}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-testid="remote-desktop-clipboard-toggle"
            aria-pressed={clipboardEnabled}
            aria-label={intl.formatMessage({
              id: clipboardEnabled ? "remoteDesktop.clipboard.on" : "remoteDesktop.clipboard.off",
            })}
            disabled={view.phase !== "connected"}
            className={cn(
              "gap-1 text-ui-sm",
              clipboardEnabled ? "text-foreground" : "text-foreground-subtle",
            )}
            onClick={() => {
              setClipboardEnabled(!clipboardEnabled);
              refocusContainer();
            }}
          >
            {clipboardEnabled ? (
              <ClipboardCheck className="size-4" />
            ) : (
              <ClipboardX className="size-4" />
            )}
            <span className="hidden sm:inline">
              {intl.formatMessage({ id: "remoteDesktop.clipboard.label" })}
            </span>
          </Button>
          {/* 画质档位（P1-3 入口 2026-10-13 移入会话面操作栏）：未连接/改档重建中禁用；
              改档 = bye + 以新档重建（fps/maxWidth 会话建立时定型，spec 画质档位节）。 */}
          <Select
            value={quality}
            disabled={view.phase !== "connected" || view.mediaStatus === "reconnecting"}
            onValueChange={(next) => {
              // 页级记忆回写由 hook 内部经 onQualityChanged 完成；此处只发起重建。
              void switchQuality(next as RemoteDesktopQuality);
              refocusContainer();
            }}
          >
            <SelectTrigger
              size="sm"
              aria-label={intl.formatMessage({ id: "remoteDesktop.quality.label" })}
              className="w-fit"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectItem value="sharp">
                {intl.formatMessage({ id: "remoteDesktop.quality.sharp" })}
              </SelectItem>
              <SelectItem value="balanced">
                {intl.formatMessage({ id: "remoteDesktop.quality.balanced" })}
              </SelectItem>
              <SelectItem value="smooth">
                {intl.formatMessage({ id: "remoteDesktop.quality.smooth" })}
              </SelectItem>
            </SelectContent>
          </Select>
          {view.monitors.length > 0 ? (
            <Select
              value={view.selectedDisplayId ?? undefined}
              disabled={view.phase === "closed"}
              onValueChange={(displayId) => {
                selectMonitor(displayId);
                // 选完把焦点还给画面（键盘输入继续直达对端）。
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
        {/* 原生媒体面：解码即画（canvas 内在尺寸 = 解码帧尺寸，object-contain 与 video 同款 letterbox）；
            cursor-none：画面内只显示对端随帧光标（spec 产品规则 7）。 */}
        <canvas ref={canvasRef} className="h-full w-full cursor-none object-contain" />
        {/* mediaStatus=live 只表示媒体面对象已建（早于任何一帧），单凭它撤掉提示会让
            「连上但一帧不出」呈现为无提示纯黑——首帧看门狗标出的 awaitingFirstFrame
            必须继续占住提示位（spec「媒体面可观测性与首帧约束」2026-10-14）。
            冻结态同理：画面停在最后一帧，中央不提示会被读成「卡死」（spec 产品规则
            「画面冻结并提示」），故一并占位（胶囊 pointer-events-none，不挡输入）。 */}
        {view.mediaStatus !== "live" || view.awaitingFirstFrame || view.frozen ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
            <div className="flex items-center gap-2 rounded-lg border border-border bg-popover/95 px-3 py-2 text-ui-base text-foreground shadow-sm">
              {view.phase === "closed" ? (
                <Unplug className="size-4 text-destructive" />
              ) : view.frozen ? (
                <Lock className="size-4 text-warning" />
              ) : (
                <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
              )}
              <span>{intl.formatMessage({ id: statusText })}</span>
            </div>
          </div>
        ) : null}
        {/* 改档重建瞬态（closed && reconnecting）不出关闭层：由「正在恢复」胶囊覆盖，
            真实失败（onGiveUp 已把 mediaStatus 收敛为 failed）与正常结束仍走本层。 */}
        {view.phase === "closed" && view.mediaStatus !== "reconnecting" ? (
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
        {stats.bitrateKbps !== null ? (
          <span>
            {intl.formatMessage(
              { id: "remoteDesktop.session.bitrate" },
              { value: (stats.bitrateKbps / 1000).toFixed(1) },
            )}
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
        {stats.frameAgeMs !== null ? (
          <span>
            {intl.formatMessage(
              { id: "remoteDesktop.session.frameAge" },
              { value: String(stats.frameAgeMs) },
            )}
          </span>
        ) : null}
        {/* 丢帧只在真的发生时提示（画面质量劣化的直接解释；无丢帧不占位）。 */}
        {stats.decodeDrops > 0 ? (
          <span className="text-warning">
            {intl.formatMessage(
              { id: "remoteDesktop.session.drops" },
              { count: String(stats.decodeDrops) },
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
