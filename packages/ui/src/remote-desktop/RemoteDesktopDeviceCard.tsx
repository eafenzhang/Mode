/**
 * 远程控制设备页 · 单张设备卡（纯展示 + 卡片内交互状态）。
 * 三段结构（docs/specs/remote-desktop-system-rdp.md 产品规则 2/8/9）：
 *   ① 头部 = 状态胶囊 + 设备名 + `host:port`；重命名时原位切换输入框；
 *   ② 主体 = 固定 16:9 连接块，整块即「进入桌面」入口（不可用时原位给原因，块高固定，
 *      所以置灰原因的出现/消失不会引起卡片高度跳变）；
 *   ③ 底部 = 设备操作行（重新检测 / 连接选项 / 重命名 / 删除；删除两段确认）。
 *
 * 连接只是「启动系统 RDP 客户端」：卡片不渲染任何画面，连接成功与否看外部 mstsc 窗口。
 * 状态胶囊与可用性都是探测结果的纯投影，卡内不维护第二份状态；连接选项草稿是纯编辑态，
 * 持久化真源在主控 main（`rdGetRdpOptions`/`rdSetRdpOptions`），故不构成重复状态源。
 */
import { useCallback, useState, type ReactNode } from "react";
import {
  ArrowRight,
  Check,
  MonitorSmartphone,
  Pencil,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
  WifiOff,
} from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import type { RdpConnectionOptions } from "@mode/shared/remote-desktop-rdp";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  resolveRemoteDesktopDeviceStatusPill,
  type RemoteDesktopDeviceProbe,
  type RemoteDesktopDeviceStatusTone,
} from "@/remote-desktop/deviceAvailability.js";
import {
  RDP_OPTION_DEFAULTS,
  resolveRdpOptions,
  toRdpConnectionOptions,
  type ResolvedRdpOptions,
} from "@/remote-desktop/rdpOptions.js";
import { RemoteDesktopConnectOptions } from "@/remote-desktop/RemoteDesktopConnectOptions.js";

/** 状态语义 → 胶囊色调（语义投影在 deviceAvailability.ts，可纯函数单测）。 */
const STATUS_PILL_TONES: Record<
  RemoteDesktopDeviceStatusTone,
  { pillClass: string; dotClass: string }
> = {
  ready: { pillClass: "bg-success text-success-foreground", dotClass: "bg-success-foreground" },
  attention: { pillClass: "bg-warning/15 text-warning", dotClass: "bg-warning" },
  muted: { pillClass: "bg-surface text-foreground-subtle", dotClass: "bg-foreground-subtlest" },
};

function DeviceStatusPill({ probe }: { probe: RemoteDesktopDeviceProbe | undefined }) {
  const { intl } = useModeIntl();
  const pill = resolveRemoteDesktopDeviceStatusPill(probe);
  const tone = STATUS_PILL_TONES[pill.tone];
  // 探测中是瞬态：只有这一档闪烁，稳定态不闪。
  const isProbing = !probe || probe.status === "probing";
  return (
    <span
      className={cn(
        "inline-flex h-5 shrink-0 items-center gap-1.5 rounded-full px-2 text-ui-xs font-medium",
        tone.pillClass,
      )}
    >
      <span className={cn("size-1.5 rounded-full", tone.dotClass, isProbing && "animate-pulse")} />
      {intl.formatMessage({ id: pill.labelId })}
    </span>
  );
}

/** 底部操作行的单个动作：图标 + 文案等宽平铺，行内不留按钮外观（对齐同类产品卡片）。 */
function DeviceActionButton({
  icon,
  label,
  testId,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  testId: string;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      data-testid={testId}
      disabled={disabled}
      onClick={onClick}
      className="flex min-w-0 flex-1 items-center justify-center gap-1.5 py-2.5 text-ui-sm text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
    >
      <span className="shrink-0">{icon}</span>
      <span className="truncate">{label}</span>
    </button>
  );
}

export function RemoteDesktopDeviceCard({
  peer,
  probe,
  busy,
  connecting,
  onRename,
  onDelete,
  onReprobe,
  onConnect,
  onError,
}: {
  peer: LanPairedPeer;
  probe: RemoteDesktopDeviceProbe | undefined;
  /** 父级操作互斥（重命名/删除/连接共用同一个 busy；卡片内不自建第二份并发控制）。 */
  busy: boolean;
  /** 本设备是否正在发起连接（父级连接态投影，卡内不猜）。 */
  connecting: boolean;
  onRename: (serverId: string, name: string) => Promise<void>;
  onDelete: (serverId: string) => Promise<void>;
  /** 单卡重新探测能力位（探测中由探测状态自身互斥，不占父级 busy）。 */
  onReprobe: (serverId: string) => void;
  /**
   * 发起连接。不带 options = 用 main 里已存的选项；带 options = 先持久化再连接
   * （连接选项面板的「确认」路径）。
   */
  onConnect: (peer: LanPairedPeer, options?: RdpConnectionOptions) => void;
  onError: (message: string) => void;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [editing, setEditing] = useState<{ name: string } | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [draftOptions, setDraftOptions] = useState<ResolvedRdpOptions | null>(null);
  const [optionsLoading, setOptionsLoading] = useState(false);

  const name = resolveRemoteDesktopDeviceLabel(peer);
  const address = peer.host && peer.port ? `${peer.host}:${peer.port}` : peer.serverId;
  const availability = resolveRemoteDesktopDeviceAvailability(probe);
  const blockedReason = availability.reason;
  const probing = !probe || probe.status === "probing";
  const offline = blockedReason === "offline" || blockedReason === "probe-failed";

  const saveRename = async () => {
    const nextName = editing?.name.trim() ?? "";
    if (!nextName) {
      return;
    }
    try {
      await onRename(peer.serverId, nextName);
      setEditing(null);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };

  /** 打开连接选项面板：读 main 里该设备已存选项作为草稿（读失败按缺省值编辑）。 */
  const openOptions = useCallback(
    (open: boolean) => {
      setOptionsOpen(open);
      if (!open) {
        setDraftOptions(null);
        return;
      }
      setOptionsLoading(true);
      setDraftOptions(RDP_OPTION_DEFAULTS);
      void platform
        .rdGetRdpOptions(peer.serverId)
        .then((stored) => setDraftOptions(resolveRdpOptions(stored)))
        .catch((error) => {
          logger.warn(
            `[remote-desktop] 读取连接选项失败 serverId=${peer.serverId}，按缺省值编辑:`,
            error instanceof Error ? error.message : String(error),
          );
        })
        .finally(() => setOptionsLoading(false));
    },
    [peer.serverId, platform],
  );

  return (
    /* overflow-hidden：底部操作行的 hover 底色要按卡片圆角裁切。 */
    <li className="flex flex-col overflow-hidden rounded-xl border border-border bg-card transition-colors hover:border-input-border-hover">
      <div className="flex min-w-0 flex-col gap-3 p-3">
        {editing ? (
          <div className="flex flex-col gap-2">
            <Input
              size="lg"
              data-testid="remote-desktop-device-name-input"
              value={editing.name}
              onChange={(event) => setEditing({ name: event.target.value })}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void saveRename();
                }
                if (event.key === "Escape") {
                  setEditing(null);
                }
              }}
            />
            <span className="flex items-center gap-2">
              <Button
                type="button"
                size="sm"
                disabled={busy || !editing.name.trim()}
                onClick={() => void saveRename()}
              >
                <Check className="size-4" />
                {intl.formatMessage({ id: "remoteDesktop.device.save" })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => setEditing(null)}
              >
                {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
              </Button>
            </span>
          </div>
        ) : (
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <DeviceStatusPill probe={probe} />
              <span className="min-w-0 flex-1 truncate text-ui-base font-semibold text-foreground">
                {name}
              </span>
            </div>
            <p className="mt-1 truncate font-mono text-ui-xs text-foreground-subtlest">{address}</p>
          </div>
        )}

        {/* 连接入口：整块预览即按钮。块高由 16:9 比例锁定，置灰原因在原位出现/消失
            都不会改变卡片高度。成功后画面在外部 mstsc 窗口，这里不渲染任何画面。 */}
        <button
          type="button"
          data-testid="remote-desktop-device-open"
          disabled={!availability.canConnect || busy}
          onClick={() => onConnect(peer)}
          className={cn(
            "group/enter flex aspect-[16/9] w-full flex-col items-center justify-center gap-2 overflow-hidden rounded-lg border border-border bg-surface px-4 text-center transition-colors",
            availability.canConnect && !busy
              ? "hover:border-input-border-hover hover:bg-surface-hover"
              : "cursor-not-allowed",
          )}
        >
          {offline ? (
            <WifiOff className="size-7 text-foreground-subtlest" />
          ) : (
            <MonitorSmartphone className="size-7 text-foreground-subtlest" />
          )}
          <span
            className={cn(
              "inline-flex items-center gap-1.5 text-ui-base font-medium",
              availability.canConnect ? "text-foreground" : "text-foreground-subtlest",
            )}
          >
            {connecting
              ? intl.formatMessage({ id: "remoteDesktop.device.opening" })
              : intl.formatMessage({ id: "remoteDesktop.device.open" })}
            <ArrowRight
              className={cn(
                "size-4",
                availability.canConnect &&
                  !busy &&
                  "transition-transform group-hover/enter:translate-x-0.5",
              )}
            />
          </span>
          {blockedReason ? (
            // line-clamp：块高被 16:9 锁死，长文案（i18n 展开）按两行截断而不是溢出块外。
            <span className="line-clamp-2 text-ui-xs text-foreground-subtle">
              {intl.formatMessage({ id: `remoteDesktop.device.reason.${blockedReason}` })}
            </span>
          ) : (
            <span className="line-clamp-2 text-ui-xs text-foreground-subtle">
              {intl.formatMessage({ id: "remoteDesktop.device.openHint" })}
            </span>
          )}
        </button>
      </div>

      {editing ? null : confirmingDelete ? (
        /* 两段确认：替换操作行，不弹二次对话框。 */
        <div className="mt-auto flex items-center justify-between gap-2 border-t border-border px-3 py-2">
          <span className="min-w-0 truncate text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.device.deleteAsk" }, { name })}
          </span>
          <span className="flex shrink-0 items-center gap-1.5">
            <Button
              type="button"
              variant="destructive"
              size="sm"
              disabled={busy}
              onClick={() => {
                void (async () => {
                  try {
                    await onDelete(peer.serverId);
                    setConfirmingDelete(false);
                  } catch (error) {
                    onError(error instanceof Error ? error.message : String(error));
                  }
                })();
              }}
            >
              {intl.formatMessage({ id: "remoteDesktop.device.deleteConfirm" })}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => setConfirmingDelete(false)}
            >
              {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
            </Button>
          </span>
        </div>
      ) : (
        <div className="mt-auto flex items-stretch border-t border-border">
          <DeviceActionButton
            testId="remote-desktop-device-reprobe"
            icon={<RefreshCw className={cn("size-3.5", probing && "animate-spin")} />}
            label={intl.formatMessage({ id: "remoteDesktop.device.reprobe" })}
            disabled={busy || probing}
            onClick={() => onReprobe(peer.serverId)}
          />
          <span aria-hidden="true" className="w-px shrink-0 self-stretch bg-border" />
          {/* 连接选项：按设备记忆（读 main 已存值），确认即持久化 + 发起连接。 */}
          <Popover open={optionsOpen} onOpenChange={openOptions}>
            <PopoverTrigger asChild>
              <button
                type="button"
                data-testid="remote-desktop-device-options"
                disabled={busy || !availability.canConnect}
                className="flex min-w-0 flex-1 items-center justify-center gap-1.5 py-2.5 text-ui-sm text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
              >
                <SlidersHorizontal className="size-3.5 shrink-0" />
                <span className="truncate">
                  {intl.formatMessage({ id: "remoteDesktop.device.options" })}
                </span>
              </button>
            </PopoverTrigger>
            <PopoverContent align="end" className="w-auto">
              {draftOptions ? (
                <RemoteDesktopConnectOptions
                  options={draftOptions}
                  busy={busy || optionsLoading}
                  onChange={setDraftOptions}
                  onConfirm={() => {
                    setOptionsOpen(false);
                    onConnect(peer, toRdpConnectionOptions(draftOptions));
                  }}
                  onCancel={() => setOptionsOpen(false)}
                />
              ) : (
                <span className="text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.options.loading" })}
                </span>
              )}
            </PopoverContent>
          </Popover>
          <span aria-hidden="true" className="w-px shrink-0 self-stretch bg-border" />
          <DeviceActionButton
            testId="remote-desktop-device-rename"
            icon={<Pencil className="size-3.5" />}
            label={intl.formatMessage({ id: "remoteDesktop.device.rename" })}
            disabled={busy}
            onClick={() => {
              onError("");
              setEditing({ name });
            }}
          />
          <span aria-hidden="true" className="w-px shrink-0 self-stretch bg-border" />
          <DeviceActionButton
            testId="remote-desktop-device-delete"
            icon={<Trash2 className="size-3.5" />}
            label={intl.formatMessage({ id: "remoteDesktop.device.delete" })}
            disabled={busy}
            onClick={() => {
              onError("");
              setConfirmingDelete(true);
            }}
          />
        </div>
      )}
    </li>
  );
}
