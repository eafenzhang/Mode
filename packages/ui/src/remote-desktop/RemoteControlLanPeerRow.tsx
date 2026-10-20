import { useState } from "react";
import { ArrowRight, Check, Globe, Pencil, RefreshCw, Trash2 } from "lucide-react";
import { formatLanPeerEndpoint, parseLanPublicBaseUrl, type LanPairedPeer } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  resolveRemoteDesktopDeviceStatusPill,
  type RemoteDesktopDeviceProbe,
} from "@/remote-desktop/deviceAvailability.js";
import {
  DeviceIdentity,
  RowShell,
  StatusPill,
} from "@/remote-desktop/RemoteControlDeviceRowParts.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

export function RemoteControlLanPeerRow({
  peer,
  probe,
  presentation,
  locallyControlled,
  busy,
  onConnect,
  onRename,
  onDelete,
  onSetPublicAddress,
  onReprobe,
  onError,
}: {
  peer: LanPairedPeer;
  probe: RemoteDesktopDeviceProbe | undefined;
  presentation?: DeviceViewMode;
  locallyControlled: boolean;
  busy: boolean;
  onConnect: (peer: LanPairedPeer) => void;
  onRename: (serverId: string, name: string) => Promise<void>;
  onDelete: (serverId: string) => Promise<void>;
  onSetPublicAddress: (serverId: string, publicBaseUrl: string | null) => Promise<void>;
  onReprobe: (serverId: string) => void;
  onError: (message: string) => void;
}) {
  const { intl } = useModeIntl();
  const [editingName, setEditingName] = useState<string | null>(null);
  const [editingPublic, setEditingPublic] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const name = resolveRemoteDesktopDeviceLabel(peer);
  const publicEndpoint = peer.endpoints?.find((endpoint) => endpoint.kind === "public");
  const primaryEndpoint = peer.endpoints?.[0];
  const primaryAddress = primaryEndpoint
    ? formatLanPeerEndpoint(primaryEndpoint)
    : peer.host && peer.port
      ? `${peer.host}:${peer.port}`
      : peer.serverId;
  const address =
    publicEndpoint && primaryEndpoint?.kind === "lan"
      ? `${primaryAddress} · ${intl.formatMessage({
          id: "remoteDesktop.device.publicEndpoint",
        })} ${formatLanPeerEndpoint(publicEndpoint)}`
      : primaryAddress;
  const availability = resolveRemoteDesktopDeviceAvailability(probe, { locallyControlled });
  const statusPill = resolveRemoteDesktopDeviceStatusPill(probe);
  const probing = !probe || probe.status === "probing";
  // 探测命中路由只用于详情文案；能力位判定仍走 availability（spec 产品规则 6）。
  const routeReason =
    probe?.status === "ok" && probe.capability.online
      ? probe.capability.endpointKind === "public"
        ? intl.formatMessage({ id: "remoteDesktop.device.publicRoute" })
        : probe.capability.endpointKind === "lan"
          ? intl.formatMessage({ id: "remoteDesktop.device.lanRoute" })
          : undefined
      : undefined;
  const reason = availability.reason
    ? intl.formatMessage({ id: `remoteDesktop.device.reason.${availability.reason}` })
    : (routeReason ?? intl.formatMessage({ id: "remoteDesktop.devices.lanDetail" }));

  const saveRename = async () => {
    const next = editingName?.trim() ?? "";
    if (!next) return;
    try {
      await onRename(peer.serverId, next);
      setEditingName(null);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };

  const savePublicAddress = async () => {
    const next = editingPublic?.trim() ?? "";
    if (next && !parseLanPublicBaseUrl(next)) {
      onError(intl.formatMessage({ id: "remoteDesktop.device.publicAddressInvalid" }));
      return;
    }
    try {
      await onSetPublicAddress(peer.serverId, next || null);
      setEditingPublic(null);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };

  const editor =
    editingPublic !== null ? (
      <div className="space-y-2">
        <p className="text-ui-xs text-foreground-subtle">
          {intl.formatMessage({ id: "remoteDesktop.device.publicAddressHint" })}
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            size="lg"
            data-testid="remote-control-lan-peer-public-input"
            value={editingPublic}
            placeholder="https://rd.example.com"
            onChange={(event) => setEditingPublic(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void savePublicAddress();
              if (event.key === "Escape") setEditingPublic(null);
            }}
          />
          <Button
            type="button"
            size="sm"
            disabled={busy}
            data-testid="remote-control-lan-peer-public-save"
            onClick={() => void savePublicAddress()}
          >
            <Check className="size-4" />
            {intl.formatMessage({ id: "remoteDesktop.device.save" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => setEditingPublic(null)}
          >
            {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
          </Button>
        </div>
      </div>
    ) : editingName ? (
      <div className="flex flex-wrap items-center gap-2">
        <Input
          size="lg"
          data-testid="remote-control-lan-peer-name-input"
          value={editingName}
          onChange={(event) => setEditingName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void saveRename();
            if (event.key === "Escape") setEditingName(null);
          }}
        />
        <Button
          type="button"
          size="sm"
          disabled={busy || !editingName.trim()}
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
          onClick={() => setEditingName(null)}
        >
          {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
        </Button>
      </div>
    ) : undefined;

  return (
    <RowShell testId="remote-control-lan-peer" presentation={presentation} editor={editor}>
      <DeviceIdentity
        pill={<StatusPill labelId={statusPill.labelId} tone={statusPill.tone} pulsing={probing} />}
        name={name}
        kindLabelId="remoteDesktop.devices.kind.lan"
        address={address}
        detail={reason}
      />
      {editingName || editingPublic !== null ? null : confirmingDelete ? (
        <div className="flex flex-wrap items-center justify-end gap-2">
          <span className="text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.device.deleteAsk" }, { name })}
          </span>
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
        </div>
      ) : (
        <div className="flex flex-wrap items-center justify-end gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="remote-control-lan-peer-open"
            disabled={!availability.canConnect}
            title={availability.canConnect ? undefined : reason}
            onClick={() => onConnect(peer)}
          >
            <ArrowRight className="size-4" />
            {intl.formatMessage({ id: "remoteDesktop.device.open" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-lan-peer-reprobe"
            title={intl.formatMessage({ id: "remoteDesktop.device.reprobe" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.device.reprobe" })}
            disabled={busy || probing}
            onClick={() => onReprobe(peer.serverId)}
          >
            <RefreshCw className={cn("size-3.5", probing && "animate-spin")} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-lan-peer-public"
            title={intl.formatMessage({ id: "remoteDesktop.device.publicAddress" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.device.publicAddress" })}
            disabled={busy}
            onClick={() => {
              onError("");
              setEditingName(null);
              setEditingPublic(publicEndpoint ? formatLanPeerEndpoint(publicEndpoint) : "");
            }}
          >
            <Globe className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-lan-peer-rename"
            title={intl.formatMessage({ id: "remoteDesktop.device.rename" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.device.rename" })}
            disabled={busy}
            onClick={() => {
              onError("");
              setEditingPublic(null);
              setEditingName(name);
            }}
          >
            <Pencil className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-lan-peer-delete"
            title={intl.formatMessage({ id: "remoteDesktop.device.delete" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.device.delete" })}
            disabled={busy}
            onClick={() => {
              onError("");
              setConfirmingDelete(true);
            }}
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      )}
    </RowShell>
  );
}
