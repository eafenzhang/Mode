import type { RemoteV3DeviceSummary } from "@mode/shared";
import { RemoteControlV3Row } from "@/remote-desktop/RemoteControlV3Row.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

/**
 * 远程控制页唯一设备列表（docs/specs/remote-control-unified-pairing.md R2）：
 * 只渲染同账号 v3 Host，行内动作固定为「进入桌面」。
 */
export function RemoteControlDeviceList({
  hosts,
  viewMode,
  locallyControlled,
  onConnect,
}: {
  hosts: RemoteV3DeviceSummary[];
  viewMode: DeviceViewMode;
  locallyControlled: boolean;
  onConnect: (host: RemoteV3DeviceSummary) => void;
}) {
  return (
    <ul
      className={
        viewMode === "grid"
          ? "grid gap-3 sm:grid-cols-2"
          : "divide-y divide-border overflow-hidden rounded-xl border border-border bg-card"
      }
      data-testid="remote-control-device-list"
      data-view={viewMode}
    >
      {hosts.map((host) => (
        <RemoteControlV3Row
          key={host.deviceId}
          host={host}
          presentation={viewMode}
          locallyControlled={locallyControlled}
          onConnect={onConnect}
        />
      ))}
    </ul>
  );
}
