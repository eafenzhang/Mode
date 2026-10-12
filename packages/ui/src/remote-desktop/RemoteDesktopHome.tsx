/**
 * 远程桌面设备页（docs/specs/lan-remote-desktop.md UI 规则「主视图 · 设备页」）：
 * 列出既有 LAN 配对设备（`getLanPairedPeers`），逐设备拉能力位（`rdGetPeerCapability`）；
 * 能力位缺失/离线/旧对端缺键 → 入口置灰并给出稳定文案；未配对 → 空态引导跳「设置 · 局域网访问」。
 *
 * 能力位不缓存：每次进入设备页与点「刷新」都重新探测（旧对端升级后立即生效）。
 */
import { useCallback, useEffect, useState } from "react";
import {
  ArrowRight,
  LoaderIcon,
  MonitorSmartphone,
  RefreshCw,
  Settings2,
  WifiOff,
} from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { AutomationsMainBreadcrumbFrame } from "@/settings/AutomationsMainBreadcrumbFrame.js";
import { logger } from "@/logger.js";
import {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  type RemoteDesktopDeviceProbe,
} from "@/remote-desktop/deviceAvailability.js";
import { RemoteDesktopSession } from "@/remote-desktop/RemoteDesktopSession.js";

type PeerListState =
  | { status: "loading" }
  | { status: "ok"; peers: LanPairedPeer[] }
  | { status: "error" };

export function RemoteDesktopHome({ isDesktop }: { isDesktop: boolean }) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  // 与其它「跳设置分区」入口同款：先写入待消费分区意图，再由 tab store 打开设置 tab。
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [peers, setPeers] = useState<PeerListState>({ status: "loading" });
  const [probes, setProbes] = useState<Record<string, RemoteDesktopDeviceProbe>>({});
  const [sessionTarget, setSessionTarget] = useState<{ serverId: string; name: string } | null>(
    null,
  );

  const loadPeers = useCallback(async () => {
    setPeers({ status: "loading" });
    try {
      const list = await platform.getLanPairedPeers();
      setPeers({ status: "ok", peers: list });
      // 逐设备探测能力位：先全部置 probing，再各自覆盖（互不阻塞）。
      const initial: Record<string, RemoteDesktopDeviceProbe> = {};
      for (const peer of list) {
        initial[peer.serverId] = { status: "probing" };
      }
      setProbes(initial);
      await Promise.all(
        list.map(async (peer) => {
          try {
            const capability = await platform.rdGetPeerCapability(peer.serverId);
            setProbes((previous) => ({
              ...previous,
              [peer.serverId]: { status: "ok", capability },
            }));
          } catch (error) {
            logger.warn(
              `[remote-desktop] 设备能力位探测失败 serverId=${peer.serverId}:`,
              error instanceof Error ? error.message : String(error),
            );
            setProbes((previous) => ({ ...previous, [peer.serverId]: { status: "probe-failed" } }));
          }
        }),
      );
    } catch (error) {
      logger.warn(
        "[remote-desktop] 读取已配对设备失败:",
        error instanceof Error ? error.message : String(error),
      );
      setPeers({ status: "error" });
    }
  }, [platform]);

  useEffect(() => {
    void loadPeers();
  }, [loadPeers]);

  const openLanAccessSettings = useCallback(() => {
    setPendingSettingsSectionIntent("lanAccess");
    openSettingsTab();
  }, [openSettingsTab]);

  // 发起会话由会话面自己完成（先订阅 `rd/session-event` 再 rdStartSession）：
  // main 在「WS 建立 → connected」推送后需要 renderer 立刻回 hello，设备页先起会话
  // 再挂载会话面会丢掉这条推送，握手就停在等 monitors 上。
  const connect = useCallback((peer: LanPairedPeer) => {
    setSessionTarget({ serverId: peer.serverId, name: resolveRemoteDesktopDeviceLabel(peer) });
  }, []);

  const exitSession = useCallback(() => {
    setSessionTarget(null);
    // 会话可能改变了对端状态（开关被关/被占用），回来立刻刷新能力位与在线状态。
    void loadPeers();
  }, [loadPeers]);

  if (sessionTarget) {
    return (
      <RemoteDesktopSession
        serverId={sessionTarget.serverId}
        deviceName={sessionTarget.name}
        onExit={exitSession}
      />
    );
  }

  const peerList = peers.status === "ok" ? peers.peers : [];

  // 设备页与自动化/插件市场同款：面包屑/桌面拖拽顶栏 + 共享标题排版（spec: lan-remote-desktop UI 规则）。
  // 会话面（上方 sessionTarget 分支）保持整块无外框，视频占满主视图。
  return (
    <div className="flex h-full min-h-0 flex-col">
      <AutomationsMainBreadcrumbFrame
        isDesktop={isDesktop}
        sectionLabel={intl.formatMessage({ id: "remoteDesktop.title" })}
        ariaLabel={intl.formatMessage({ id: "remoteDesktop.breadcrumbLabel" })}
      >
        <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
          <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 px-4 py-4 md:px-6 md:py-6">
            <header className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0">
                <h1 className="text-2xl font-semibold tracking-tight text-foreground lg:text-3xl">
                  {intl.formatMessage({ id: "remoteDesktop.title" })}
                </h1>
                <p className="mt-1 text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.description" })}
                </p>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={peers.status === "loading"}
                onClick={() => void loadPeers()}
              >
                <RefreshCw className="size-4" />
                {intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
              </Button>
            </header>

            {peers.status === "loading" ? (
              <p className="flex items-center gap-2 py-6 text-ui-base text-foreground-subtle">
                <LoaderIcon className="size-4 animate-spin" />
                {intl.formatMessage({ id: "remoteDesktop.devices.loading" })}
              </p>
            ) : null}

            {peers.status === "error" ? (
              <div className="rounded-lg border border-border bg-card px-4 py-6 text-center">
                <p className="text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.devices.loadFailed" })}
                </p>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-3"
                  onClick={() => void loadPeers()}
                >
                  {intl.formatMessage({ id: "remoteDesktop.devices.retry" })}
                </Button>
              </div>
            ) : null}

            {peers.status === "ok" && peerList.length === 0 ? (
              <div
                data-testid="remote-desktop-empty"
                className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border bg-surface px-4 py-10 text-center"
              >
                <MonitorSmartphone className="size-6 text-foreground-subtlest" />
                <p className="max-w-md text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.devices.empty" })}
                </p>
                <Button type="button" variant="outline" size="sm" onClick={openLanAccessSettings}>
                  <Settings2 className="size-4" />
                  {intl.formatMessage({ id: "remoteDesktop.devices.openLanAccess" })}
                </Button>
              </div>
            ) : null}

            {peerList.length > 0 ? (
              <ul className="grid gap-3 sm:grid-cols-2" data-testid="remote-desktop-devices">
                {peerList.map((peer) => {
                  const name = resolveRemoteDesktopDeviceLabel(peer);
                  const availability = resolveRemoteDesktopDeviceAvailability(
                    probes[peer.serverId],
                  );
                  const blockedReason = availability.reason;
                  return (
                    <li
                      key={peer.serverId}
                      className="flex flex-col gap-3 rounded-lg border border-border bg-card px-4 py-3"
                    >
                      <div className="flex items-start gap-3">
                        <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md bg-surface text-foreground-subtle">
                          {blockedReason === "offline" || blockedReason === "probe-failed" ? (
                            <WifiOff className="size-4" />
                          ) : (
                            <MonitorSmartphone className="size-4" />
                          )}
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-ui-base text-foreground">
                            {name}
                          </span>
                          <span className="block truncate text-ui-xs text-foreground-subtle">
                            {peer.host && peer.port ? `${peer.host}:${peer.port}` : peer.serverId}
                          </span>
                          <span className="mt-1 block text-ui-xs text-foreground-subtle">
                            {blockedReason
                              ? intl.formatMessage({
                                  id: `remoteDesktop.device.reason.${blockedReason}`,
                                })
                              : intl.formatMessage({ id: "remoteDesktop.device.reason.ready" })}
                          </span>
                        </span>
                      </div>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="self-end"
                        disabled={!availability.canConnect}
                        onClick={() => connect(peer)}
                      >
                        <ArrowRight className="size-4" />
                        {intl.formatMessage({ id: "remoteDesktop.devices.connect" })}
                      </Button>
                    </li>
                  );
                })}
              </ul>
            ) : null}
          </div>
        </div>
      </AutomationsMainBreadcrumbFrame>
    </div>
  );
}
