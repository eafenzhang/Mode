import { ArrowRight, ShieldCheck } from "lucide-react";
import type { RemoteV3DeviceSummary } from "@mode/shared";

import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  DeviceIdentity,
  RowShell,
  StatusPill,
} from "@/remote-desktop/RemoteControlDeviceRowParts.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

/** v3 Host 行：账号目录来源，连接走 E2EE + 本机回环媒体/信令代理。 */
export function RemoteControlV3Row({
  host,
  presentation,
  locallyControlled,
  onConnect,
}: {
  host: RemoteV3DeviceSummary;
  presentation?: DeviceViewMode;
  locallyControlled: boolean;
  onConnect: (host: RemoteV3DeviceSummary) => void;
}) {
  const { intl } = useModeIntl();
  const canConnect = host.online && !locallyControlled;
  // 目录里带地址 = 被控端开了「允许局域网直连」；仅展示是否同网段，
  // 实际走直连还是 Relay 由会话面建连时选路，行内不作能力断言。
  const address = host.addresses[0] ?? host.deviceId.slice(0, 8);
  return (
    <RowShell testId="remote-control-v3-host" presentation={presentation}>
      <DeviceIdentity
        pill={
          <StatusPill
            labelId={host.online ? "remoteDesktop.v3.online" : "remoteDesktop.v3.offline"}
            tone={host.online ? "ready" : "muted"}
          />
        }
        name={host.name}
        kindLabelId="remoteDesktop.devices.kind.v3"
        address={address}
        detail={intl.formatMessage({ id: "remoteDesktop.v3.e2ee" })}
      />
      <Button
        type="button"
        variant="outline"
        size="sm"
        data-testid="remote-control-v3-open"
        disabled={!canConnect}
        title={
          canConnect
            ? undefined
            : intl.formatMessage({
                id: locallyControlled
                  ? "remoteDesktop.device.reason.locally-controlled"
                  : "remoteDesktop.v3.offline",
              })
        }
        onClick={() => onConnect(host)}
      >
        <ShieldCheck className="size-4" aria-hidden="true" />
        <ArrowRight className="size-4" aria-hidden="true" />
        {intl.formatMessage({ id: "remoteDesktop.device.open" })}
      </Button>
    </RowShell>
  );
}
