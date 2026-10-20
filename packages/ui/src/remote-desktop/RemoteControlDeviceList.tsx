import type { LanDiscoveredPeer, LanPairedPeer } from "@mode/shared";
import type { RemoteDesktopDeviceProbe } from "@/remote-desktop/deviceAvailability.js";
import { RemoteControlDiscoveredRow } from "@/remote-desktop/RemoteControlDiscoveredRow.js";
import { RemoteControlLanPeerRow } from "@/remote-desktop/RemoteControlLanPeerRow.js";
import { RemoteControlModeAgentRow } from "@/remote-desktop/RemoteControlModeAgentRow.js";
import type { ModeAgentServerRecord } from "@/remote-desktop/modeAgentPairing.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

/**
 * 远程控制页唯一设备列表（docs/specs/remote-control-unified-pairing.md R2）：
 * LAN 已配对设备、Mode Agent 设备、发现的未配对设备共用同一个 <ul>；
 * 行内交互与状态各自成组件，列表只负责排序与转发动作。
 */
export function RemoteControlDeviceList({
  lanPeers,
  probes,
  modeAgents,
  discovered,
  viewMode,
  locallyControlled,
  busy,
  onConnect,
  onRename,
  onDelete,
  onSetPublicAddress,
  onReprobe,
  onStartChat,
  onRefreshModeAgent,
  onRenameModeAgent,
  onRemoveModeAgent,
  onPairDiscovered,
  onDismissDiscovered,
  onError,
}: {
  lanPeers: LanPairedPeer[];
  probes: Record<string, RemoteDesktopDeviceProbe>;
  modeAgents: ModeAgentServerRecord[];
  discovered: LanDiscoveredPeer[];
  viewMode: DeviceViewMode;
  locallyControlled: boolean;
  busy: boolean;
  onConnect: (peer: LanPairedPeer) => void;
  onRename: (serverId: string, name: string) => Promise<void>;
  onDelete: (serverId: string) => Promise<void>;
  onSetPublicAddress: (serverId: string, publicBaseUrl: string | null) => Promise<void>;
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
      {lanPeers.map((peer) => (
        <RemoteControlLanPeerRow
          key={`lan-${peer.serverId}`}
          peer={peer}
          probe={probes[peer.serverId]}
          presentation={viewMode}
          locallyControlled={locallyControlled}
          busy={busy}
          onConnect={onConnect}
          onRename={onRename}
          onDelete={onDelete}
          onSetPublicAddress={onSetPublicAddress}
          onReprobe={onReprobe}
          onError={onError}
        />
      ))}
      {modeAgents.map((record) => (
        <RemoteControlModeAgentRow
          key={`mode-agent-${record.serverName}`}
          record={record}
          presentation={viewMode}
          busy={busy}
          onStartChat={onStartChat}
          onRefresh={onRefreshModeAgent}
          onRename={onRenameModeAgent}
          onRemove={onRemoveModeAgent}
          onError={onError}
        />
      ))}
      {discovered.map((peer) => (
        <RemoteControlDiscoveredRow
          key={`discovered-${peer.serverId}`}
          peer={peer}
          presentation={viewMode}
          busy={busy}
          onPair={onPairDiscovered}
          onDismiss={onDismissDiscovered}
        />
      ))}
    </ul>
  );
}
