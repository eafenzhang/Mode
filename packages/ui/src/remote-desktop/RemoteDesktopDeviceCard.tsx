/**
 * 远程控制设备页 · 单张设备卡（从 RemoteDesktopHome 拆出，max-lines 约束；纯展示 + 卡片内
 * 交互状态）。三段结构（docs/specs/lan-remote-desktop.md UI 规则「设备页」，2026-10-14 改版）：
 *   ① 头部 = 状态胶囊 + 设备名 + `host:port`；重命名时原位切换输入框；
 *   ② 主体 = 固定 16:9 预览块，整块即「进入桌面」入口（不可用时原位给原因，块高固定，
 *      所以置灰原因的出现/消失不会引起卡片高度跳变）；
 *   ③ 底部 = 设备操作行（重新检测 / 重命名 / 删除；删除两段确认）。
 * 状态胶囊与可用性都是探测结果的纯投影，卡内不维护第二份状态。
 */
import { useState, type ReactNode } from "react";
import {
  ArrowRight,
  Check,
  MonitorSmartphone,
  Pencil,
  RefreshCw,
  Trash2,
  WifiOff,
} from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  resolveRemoteDesktopDeviceStatusPill,
  type RemoteDesktopDeviceProbe,
  type RemoteDesktopDeviceStatusTone,
} from "@/remote-desktop/deviceAvailability.js";

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
  locallyControlled,
  busy,
  onRename,
  onDelete,
  onReprobe,
  onConnect,
  onError,
}: {
  peer: LanPairedPeer;
  probe: RemoteDesktopDeviceProbe | undefined;
  locallyControlled: boolean;
  /** 父级操作互斥（重命名/删除共用同一个 busy；卡片内不自建第二份并发控制）。 */
  busy: boolean;
  onRename: (serverId: string, name: string) => Promise<void>;
  onDelete: (serverId: string) => Promise<void>;
  /** 单卡重新探测能力位（探测中由探测状态自身互斥，不占父级 busy）。 */
  onReprobe: (serverId: string) => void;
  /** 发起连接（画质档位已移至会话面顶部操作栏——spec: native-media「画质档位」2026-10-13）。 */
  onConnect: (peer: LanPairedPeer) => void;
  onError: (message: string) => void;
}) {
  const { intl } = useModeIntl();
  const [editing, setEditing] = useState<{ name: string } | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const name = resolveRemoteDesktopDeviceLabel(peer);
  const address = peer.host && peer.port ? `${peer.host}:${peer.port}` : peer.serverId;
  const availability = resolveRemoteDesktopDeviceAvailability(probe, { locallyControlled });
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
            都不会改变卡片高度（spec 产品规则 5 的「高度跳变」约束）。 */}
        <button
          type="button"
          data-testid="remote-desktop-device-open"
          disabled={!availability.canConnect}
          onClick={() => onConnect(peer)}
          className={cn(
            "group/enter flex aspect-[16/9] w-full flex-col items-center justify-center gap-2 overflow-hidden rounded-lg border border-border bg-surface px-4 text-center transition-colors",
            availability.canConnect
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
            {intl.formatMessage({ id: "remoteDesktop.device.open" })}
            <ArrowRight
              className={cn(
                "size-4",
                availability.canConnect && "transition-transform group-hover/enter:translate-x-0.5",
              )}
            />
          </span>
          {blockedReason ? (
            // line-clamp：块高被 16:9 锁死，长文案（i18n 展开）按两行截断而不是溢出块外。
            <span className="line-clamp-2 text-ui-xs text-foreground-subtle">
              {intl.formatMessage({ id: `remoteDesktop.device.reason.${blockedReason}` })}
            </span>
          ) : null}
        </button>
      </div>

      {editing ? null : confirmingDelete ? (
        /* 两段确认（spec UI 规则「设备页」）：替换操作行，不弹二次对话框。 */
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
