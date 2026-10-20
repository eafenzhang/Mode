import { useState } from "react";
import { LoaderIcon, Radar, X } from "lucide-react";
import type { LanDiscoveredPeer } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  DeviceIdentity,
  RowShell,
  StatusPill,
} from "@/remote-desktop/RemoteControlDeviceRowParts.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

export function RemoteControlDiscoveredRow({
  peer,
  presentation,
  busy,
  onPair,
  onDismiss,
}: {
  peer: LanDiscoveredPeer;
  presentation?: DeviceViewMode;
  busy: boolean;
  onPair: (peer: LanDiscoveredPeer, code: string) => Promise<void>;
  onDismiss: (serverId: string) => void;
}) {
  const { intl } = useModeIntl();
  const [selected, setSelected] = useState(false);
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState("");

  const pair = async () => {
    setPairing(true);
    setPairError("");
    try {
      await onPair(peer, code.trim().toUpperCase());
    } catch (error) {
      setPairError(error instanceof Error ? error.message : String(error));
    } finally {
      setPairing(false);
    }
  };

  return (
    <RowShell
      testId="remote-control-discovered"
      presentation={presentation}
      editor={
        selected ? (
          <div className="space-y-2 rounded-lg border border-dashed border-border px-3 py-2">
            <div className="flex flex-wrap items-center gap-2">
              <Input
                size="lg"
                className="max-w-40 tracking-[0.3em] uppercase"
                data-testid="remote-control-discovered-code"
                value={code}
                onChange={(event) =>
                  setCode(
                    event.target.value
                      .replace(/[^0-9a-fA-F]/gu, "")
                      .slice(0, 6)
                      .toUpperCase(),
                  )
                }
                placeholder="A1B2C3"
              />
              <Button
                type="button"
                variant="outline"
                size="lg"
                data-testid="remote-control-discovered-confirm"
                disabled={pairing || code.trim().length < 4}
                onClick={() => void pair()}
              >
                {pairing ? <LoaderIcon className="size-3.5 animate-spin" /> : null}
                {intl.formatMessage({
                  id: pairing ? "remoteDesktop.devices.pairing" : "remoteDesktop.devices.pair",
                })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="lg"
                disabled={pairing}
                onClick={() => {
                  setSelected(false);
                  setPairError("");
                }}
              >
                {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
              </Button>
            </div>
            <p className="text-ui-xs text-foreground-subtlest">
              {intl.formatMessage({ id: "remoteDesktop.devices.pairCodeHint" })}
            </p>
            {pairError ? <p className="text-ui-base text-destructive">{pairError}</p> : null}
          </div>
        ) : undefined
      }
    >
      <DeviceIdentity
        pill={<StatusPill labelId="remoteDesktop.devices.status.unpaired" tone="muted" />}
        name={peer.name?.trim() || peer.serverId}
        kindLabelId="remoteDesktop.devices.kind.lan"
        address={`${peer.host}:${peer.port}`}
        detail={intl.formatMessage({ id: "remoteDesktop.devices.discoveredHint" })}
      />
      <div className="flex flex-wrap items-center justify-end gap-1.5">
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="remote-control-discovered-pair"
          disabled={busy}
          onClick={() => {
            setSelected((previous) => !previous);
            setPairError("");
          }}
        >
          <Radar className="size-4" />
          {intl.formatMessage({ id: "remoteDesktop.devices.pair" })}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          data-testid="remote-control-discovered-dismiss"
          title={intl.formatMessage({ id: "remoteDesktop.devices.dismiss" })}
          aria-label={intl.formatMessage({ id: "remoteDesktop.devices.dismiss" })}
          disabled={busy}
          onClick={() => onDismiss(peer.serverId)}
        >
          <X className="size-3.5" />
        </Button>
      </div>
    </RowShell>
  );
}
