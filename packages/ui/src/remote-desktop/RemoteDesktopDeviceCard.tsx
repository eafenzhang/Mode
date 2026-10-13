/**
 * 远程控制设备页 · 单张设备卡（从 RemoteDesktopHome 拆出，max-lines 约束；纯展示 + 卡片内
 * 交互状态）：重命名（两段确认）、删除（两段确认）、连接入口；可用性与状态徽标都是探测
 * 结果的纯投影，不引入第二份状态（docs/specs/lan-remote-desktop.md UI 规则「设备页」）。
 */
import { useState } from "react";
import { ArrowRight, Check, MonitorSmartphone, Pencil, Trash2, WifiOff } from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  type RemoteDesktopDeviceProbe,
} from "@/remote-desktop/deviceAvailability.js";

/** 设备卡状态徽标（探测结果的纯展示投影）：色调 + i18n key，与置灰原因同源。 */
function resolveDeviceStatusBadge(probe: RemoteDesktopDeviceProbe | undefined): {
  labelId: string;
  dotClass: string;
  textClass: string;
} {
  if (!probe || probe.status === "probing") {
    return {
      labelId: "remoteDesktop.device.status.probing",
      dotClass: "bg-foreground-subtlest animate-pulse",
      textClass: "text-foreground-subtle",
    };
  }
  if (probe.status === "probe-failed") {
    return {
      labelId: "remoteDesktop.device.status.unknown",
      dotClass: "bg-foreground-subtlest",
      textClass: "text-foreground-subtle",
    };
  }
  if (!probe.capability.online) {
    return {
      labelId: "remoteDesktop.device.status.offline",
      dotClass: "bg-warning",
      textClass: "text-warning",
    };
  }
  if (!probe.capability.remoteDesktop || !probe.capability.remoteDesktopNative) {
    return {
      labelId: "remoteDesktop.device.status.unsupported",
      dotClass: "bg-warning",
      textClass: "text-warning",
    };
  }
  return {
    labelId: "remoteDesktop.device.status.ready",
    dotClass: "bg-success",
    textClass: "text-success",
  };
}

function DeviceStatusBadge({ probe }: { probe: RemoteDesktopDeviceProbe | undefined }) {
  const { intl } = useModeIntl();
  const badge = resolveDeviceStatusBadge(probe);
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full bg-surface px-2 py-0.5 text-ui-xs",
        badge.textClass,
      )}
    >
      <span className={cn("size-1.5 rounded-full", badge.dotClass)} />
      {intl.formatMessage({ id: badge.labelId })}
    </span>
  );
}

export function RemoteDesktopDeviceCard({
  peer,
  probe,
  locallyControlled,
  busy,
  onRename,
  onDelete,
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
  onConnect: (peer: LanPairedPeer) => void;
  onError: (message: string) => void;
}) {
  const { intl } = useModeIntl();
  const [editing, setEditing] = useState<{ name: string } | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const name = resolveRemoteDesktopDeviceLabel(peer);
  const availability = resolveRemoteDesktopDeviceAvailability(probe, { locallyControlled });
  const blockedReason = availability.reason;
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
    <li className="flex flex-col gap-3 rounded-xl border border-border bg-card px-4 py-3 transition-colors hover:border-input-border-hover">
      <div className="flex items-start gap-3">
        <span className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg bg-surface text-foreground-subtle">
          {offline ? <WifiOff className="size-4" /> : <MonitorSmartphone className="size-4" />}
        </span>
        {editing ? (
          <span className="min-w-0 flex-1 space-y-2">
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
          </span>
        ) : (
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate text-ui-base font-medium text-foreground">
                {name}
              </span>
              <DeviceStatusBadge probe={probe} />
            </span>
            <span className="mt-1 block truncate text-ui-xs text-foreground-subtle">
              {peer.host && peer.port ? `${peer.host}:${peer.port}` : peer.serverId}
            </span>
            {/* 置灰原因只在被挡时展示；可用态由徽标承担（不重复文案）。 */}
            {blockedReason ? (
              <span className="mt-1.5 block text-ui-xs text-foreground-subtle">
                {intl.formatMessage({ id: `remoteDesktop.device.reason.${blockedReason}` })}
              </span>
            ) : null}
          </span>
        )}
      </div>
      {editing ? null : (
        <div className="mt-auto flex items-center justify-between gap-2">
          {confirmingDelete ? (
            <>
              <span className="min-w-0 truncate text-ui-xs text-foreground-subtle">
                {intl.formatMessage(
                  { id: "remoteDesktop.device.deleteAsk" },
                  { name },
                )}
              </span>
              <span className="flex items-center gap-1.5">
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
            </>
          ) : (
            <>
              <span className="flex items-center gap-0.5">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    onError("");
                    setEditing({ name });
                  }}
                >
                  <Pencil className="size-3.5" />
                  {intl.formatMessage({ id: "remoteDesktop.device.rename" })}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    onError("");
                    setConfirmingDelete(true);
                  }}
                >
                  <Trash2 className="size-3.5" />
                  {intl.formatMessage({ id: "remoteDesktop.device.delete" })}
                </Button>
              </span>
              <Button
                type="button"
                size="sm"
                disabled={!availability.canConnect}
                onClick={() => onConnect(peer)}
              >
                <ArrowRight className="size-4" />
                {intl.formatMessage({ id: "remoteDesktop.devices.connect" })}
              </Button>
            </>
          )}
        </div>
      )}
    </li>
  );
}
