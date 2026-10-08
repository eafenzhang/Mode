import { useCallback, useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import type { LanPairedPeer, ServerRemoteWorkspaceInfo } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { toast } from "@/components/ui/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";

type PeerWorkspacesState =
  | { status: "loading" }
  | { status: "ok"; workspaces: ServerRemoteWorkspaceInfo[] }
  | { status: "error" };

/**
 * 「我配对的对端」（客户端视角，spec: docs/specs/lan-paired-devices.md）：
 * 列表来自平台接口（host 枚举凭据元数据，不暴露令牌）；每行按需拉对端
 * server-info 的工作区目录，失败降级为「无法获取」，不影响列表本身；
 * 删除清长期令牌与元数据，下次连接需重新配对。本机 LAN 开关关闭时也显示。
 */
export function LanAccessPairedPeersCard({ onError }: { onError: (message: string) => void }) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [peers, setPeers] = useState<LanPairedPeer[]>([]);
  const [peersLoading, setPeersLoading] = useState(true);
  const [peerBusy, setPeerBusy] = useState(false);
  const [peerWorkspaces, setPeerWorkspaces] = useState<Record<string, PeerWorkspacesState>>({});

  const loadPairedPeers = useCallback(async () => {
    setPeersLoading(true);
    try {
      const list = await platform.getLanPairedPeers();
      setPeers(list);
      const initial: Record<string, PeerWorkspacesState> = {};
      for (const peer of list) {
        initial[peer.serverId] = { status: "loading" };
      }
      setPeerWorkspaces(initial);
      await Promise.all(
        list.map(async (peer) => {
          if (!peer.host || !peer.port) {
            setPeerWorkspaces((prev) => ({ ...prev, [peer.serverId]: { status: "error" } }));
            return;
          }
          try {
            const info = await platform.getLanPeerWorkspaces(peer.serverId);
            setPeerWorkspaces((prev) => ({
              ...prev,
              [peer.serverId]: { status: "ok", workspaces: info.workspaces },
            }));
          } catch {
            setPeerWorkspaces((prev) => ({ ...prev, [peer.serverId]: { status: "error" } }));
          }
        }),
      );
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setPeersLoading(false);
    }
  }, [platform, onError]);

  useEffect(() => {
    void loadPairedPeers();
  }, [loadPairedPeers]);

  const removePairedPeer = async (peer: LanPairedPeer) => {
    setPeerBusy(true);
    try {
      await platform.removeLanPairedPeer(peer.serverId);
      await loadPairedPeers();
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      onError(message);
      toast(`${intl.formatMessage({ id: "settings.lanAccess.removeFailed" })}：${message}`);
    } finally {
      setPeerBusy(false);
    }
  };

  return (
    <SettingsGroupCard>
      <SettingsRow
        label={intl.formatMessage({ id: "settings.lanAccess.pairedPeers" })}
        description={
          peersLoading
            ? intl.formatMessage({ id: "common.loading" })
            : peers.length === 0
              ? intl.formatMessage({ id: "settings.lanAccess.noPairedPeers" })
              : intl.formatMessage(
                  { id: "settings.lanAccess.clientCount" },
                  { count: String(peers.length) },
                )
        }
        control={
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={peerBusy || peersLoading}
            onClick={() => void loadPairedPeers()}
          >
            {intl.formatMessage({ id: "settings.lanAccess.refreshPairedPeers" })}
          </Button>
        }
        detail={
          !peersLoading && peers.length > 0 ? (
            <ul className="space-y-2" data-testid="lan-paired-peers">
              {peers.map((peer) => {
                const name = peer.name?.trim() || peer.serverId;
                const workspaces = peerWorkspaces[peer.serverId];
                return (
                  <li
                    key={peer.serverId}
                    className="flex items-start justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-ui-base text-foreground">{name}</span>
                      <span className="block truncate text-ui-xs text-foreground-subtle">
                        {peer.host && peer.port
                          ? `${peer.host}:${peer.port}`
                          : intl.formatMessage({ id: "settings.lanAccess.addressUnknown" })}
                      </span>
                      <span className="mt-0.5 block text-ui-xs text-foreground-subtle">
                        {intl.formatMessage({ id: "settings.lanAccess.workspaces" })}
                        {workspaces?.status === "ok" ? (
                          workspaces.workspaces.length > 0 ? (
                            <span className="mt-0.5 block space-y-0.5">
                              {workspaces.workspaces.slice(0, 5).map((workspace) => (
                                <span
                                  key={workspace.path}
                                  className="block truncate font-mono text-ui-xs text-foreground-subtle"
                                >
                                  {workspace.path}
                                </span>
                              ))}
                              {workspaces.workspaces.length > 5 ? (
                                <span className="block text-ui-xs text-foreground-subtle">
                                  {intl.formatMessage(
                                    { id: "settings.lanAccess.moreWorkspaces" },
                                    { count: String(workspaces.workspaces.length - 5) },
                                  )}
                                </span>
                              ) : null}
                            </span>
                          ) : (
                            <span className="block text-ui-xs text-foreground-subtle">
                              {intl.formatMessage({ id: "settings.lanAccess.noWorkspaces" })}
                            </span>
                          )
                        ) : (
                          <span className="block text-ui-xs text-foreground-subtle">
                            {workspaces?.status === "loading"
                              ? intl.formatMessage({ id: "common.loading" })
                              : intl.formatMessage({
                                  id: "settings.lanAccess.workspacesUnavailable",
                                })}
                          </span>
                        )}
                      </span>
                    </span>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={peerBusy}
                      aria-label={intl.formatMessage(
                        { id: "settings.lanAccess.removeClient" },
                        { name },
                      )}
                      onClick={() => void removePairedPeer(peer)}
                    >
                      <Trash2 className="size-4" />
                    </Button>
                  </li>
                );
              })}
            </ul>
          ) : undefined
        }
      />
    </SettingsGroupCard>
  );
}
