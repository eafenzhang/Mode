import type { LanDiscoveredPeer, LanPairedPeer } from "@mode/shared";
import { RemoteControlDiscoveredRow } from "@/remote-desktop/RemoteControlDiscoveredRow.js";
import { RemoteControlLanPeerRow } from "@/remote-desktop/RemoteControlLanPeerRow.js";
import { RemoteControlModeAgentRow } from "@/remote-desktop/RemoteControlModeAgentRow.js";
import type { ModeAgentServerRecord } from "@/remote-desktop/modeAgentPairing.js";
import type { RemoteDevice } from "@/remote-desktop/remoteDeviceModel.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

/**
 * 远程控制页唯一设备列表（docs/specs/remote-control-device-model.md）：
 * 列表只消费统一 `RemoteDevice[]`；行按 `origin` 选择展示组件，动作按 `channels` 渲染。
 */
export function RemoteControlDeviceList({
  devices,
  viewMode,
  locallyControlled,
  busy,
  onConnect,
  onConnectWorkspace,
  onRename,
  onDelete,
  onReprobe,
  onStartChat,
  onRefreshModeAgent,
  onRenameModeAgent,
  onRemoveModeAgent,
  onPairDiscovered,
  onDismissDiscovered,
  onError,
}: {
  devices: RemoteDevice[];
  viewMode: DeviceViewMode;
  locallyControlled: boolean;
  busy: boolean;
  onConnect: (peer: LanPairedPeer) => void;
  onConnectWorkspace?: (peer: LanPairedPeer) => void;
  onRename: (serverId: string, name: string) => Promise<void>;
  onDelete: (serverId: string) => Promise<void>;
  onReprobe: (serverId: string) => void;
  onStartChat: (record: ModeAgentServerRecord) => Promise<void>;
  onRefreshModeAgent: (record: ModeAgentServerRecord) => Promise<void>;
  onRenameModeAgent: (record: ModeAgentServerRecord, name: string) => Promise<void>;
  onRemoveModeAgent: (record: ModeAgentServerRecord) => Promise<void>;
  onPairDiscovered: (peer: LanDiscoveredPeer, code: string) => Promise<void>;
  onDismissDiscovered: (serverId: string) => void;
  onError: (message: string) => void;
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
      {devices.map((device) => {
        if (device.origin === "lan") {
          return (
            <RemoteControlLanPeerRow
              key={`lan-${device.id}`}
              peer={device.peer}
              probe={device.probe}
              presentation={viewMode}
              locallyControlled={locallyControlled}
              busy={busy}
              onConnect={onConnect}
              onConnectWorkspace={onConnectWorkspace}
              onRename={onRename}
              onDelete={onDelete}
              onReprobe={onReprobe}
              onError={onError}
            />
          );
        }
        if (device.origin === "mode-agent") {
          return (
            <RemoteControlModeAgentRow
              key={`mode-agent-${device.id}`}
              record={device.record}
              presentation={viewMode}
              busy={busy}
              onStartChat={onStartChat}
              onRefresh={onRefreshModeAgent}
              onRename={onRenameModeAgent}
              onRemove={onRemoveModeAgent}
              onError={onError}
            />
          );
        }
        return (
          <RemoteControlDiscoveredRow
            key={`discovered-${device.id}`}
            peer={device.peer}
            presentation={viewMode}
            busy={busy}
            onPair={onPairDiscovered}
            onDismiss={onDismissDiscovered}
          />
        );
      })}
    </ul>
  );
}
