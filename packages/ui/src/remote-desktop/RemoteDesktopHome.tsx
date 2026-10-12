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
  Check,
  LoaderIcon,
  MonitorSmartphone,
  Pencil,
  RefreshCw,
  Settings2,
  Trash2,
  WifiOff,
} from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
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

export function RemoteDesktopHome({
  isDesktop,
  active,
  topOverlayClearancePaddingClass,
}: {
  isDesktop: boolean;
  /** 视图是否当前可见：分支常驻隐藏时为 false（spec UI 规则「切换保持连接」）。 */
  active: boolean;
  /** 左侧栏收起时让出顶部浮层左上入口宽度的 ps-* 类（仅会话面操作栏消费）。 */
  topOverlayClearancePaddingClass?: string | null;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  // 与其它「跳设置分区」入口同款：先写入待消费分区意图，再由 tab store 打开设置 tab。
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [peers, setPeers] = useState<PeerListState>({ status: "loading" });
  const [probes, setProbes] = useState<Record<string, RemoteDesktopDeviceProbe>>({});
  const [locallyControlled, setLocallyControlled] = useState(false);
  const [sessionTarget, setSessionTarget] = useState<{ serverId: string; name: string } | null>(
    null,
  );
  // 设备管理（编辑/删除）：编辑 = 重命名本机记录（renameLanPairedPeer，不动对端与令牌）；
  // 删除 = 清配对令牌与元数据（removeLanPairedPeer，两段式确认防误触）。
  const [editingPeer, setEditingPeer] = useState<{ serverId: string; name: string } | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [peerActionBusy, setPeerActionBusy] = useState(false);
  const [peerActionError, setPeerActionError] = useState("");

  const loadPeers = useCallback(async () => {
    setPeers({ status: "loading" });
    // 本机被控状态与设备列表并行读（spec 产品规则 9）；读失败按未被控展示，
    // 发起瞬间仍由 main 的 rd/start-session 权威拦截（locally_controlled）。
    void platform
      .rdGetControlledState()
      .then(({ controlled }) => setLocallyControlled(controlled))
      .catch(() => undefined);
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

  // 首次挂载即在视图内；之后只在「重新可见」或「会话结束回到设备列表」时刷新探测——
  // 分支常驻隐藏期间不发网络请求。会话进行中（sessionTarget 非空）不刷新。
  useEffect(() => {
    if (active && !sessionTarget) {
      void loadPeers();
    }
  }, [active, sessionTarget, loadPeers]);

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
    // 回设备列表的刷新由上方 active/sessionTarget effect 统一触发（含本机被控状态重读）。
    setSessionTarget(null);
  }, []);

  /** 保存重命名（编辑）：只改本机记录的名称；成功后刷新列表复位编辑态。 */
  const saveRename = useCallback(async () => {
    const target = editingPeer;
    const name = target?.name.trim() ?? "";
    if (!target || !name) {
      return;
    }
    setPeerActionBusy(true);
    setPeerActionError("");
    try {
      await platform.renameLanPairedPeer(target.serverId, name);
      setEditingPeer(null);
      await loadPeers();
    } catch (error) {
      setPeerActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setPeerActionBusy(false);
    }
  }, [editingPeer, loadPeers, platform]);

  /** 删除（两段式确认后执行）：清配对令牌与元数据，成功后刷新列表。 */
  const deletePeer = useCallback(
    async (serverId: string) => {
      setPeerActionBusy(true);
      setPeerActionError("");
      try {
        await platform.removeLanPairedPeer(serverId);
        setPendingDeleteId(null);
        await loadPeers();
      } catch (error) {
        setPeerActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setPeerActionBusy(false);
      }
    },
    [loadPeers, platform],
  );

  if (sessionTarget) {
    return (
      <RemoteDesktopSession
        serverId={sessionTarget.serverId}
        deviceName={sessionTarget.name}
        topOverlayClearancePaddingClass={topOverlayClearancePaddingClass}
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

            {peerActionError ? (
              <p className="rounded-lg border border-border bg-card px-3 py-2 text-ui-base text-destructive">
                {peerActionError}
              </p>
            ) : null}

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
                    { locallyControlled },
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
                        {editingPeer?.serverId === peer.serverId ? (
                          <span className="min-w-0 flex-1 space-y-2">
                            <Input
                              size="lg"
                              data-testid="remote-desktop-device-name-input"
                              value={editingPeer.name}
                              onChange={(event) =>
                                setEditingPeer({ serverId: peer.serverId, name: event.target.value })
                              }
                              onKeyDown={(event) => {
                                if (event.key === "Enter") {
                                  void saveRename();
                                }
                                if (event.key === "Escape") {
                                  setEditingPeer(null);
                                }
                              }}
                            />
                            <span className="flex items-center gap-2">
                              <Button
                                type="button"
                                size="sm"
                                disabled={peerActionBusy || !editingPeer.name.trim()}
                                onClick={() => void saveRename()}
                              >
                                <Check className="size-4" />
                                {intl.formatMessage({ id: "remoteDesktop.device.save" })}
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                disabled={peerActionBusy}
                                onClick={() => setEditingPeer(null)}
                              >
                                {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
                              </Button>
                            </span>
                          </span>
                        ) : (
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
                        )}
                      </div>
                      {editingPeer?.serverId === peer.serverId ? null : (
                        <div className="flex items-center justify-end gap-1.5">
                          {pendingDeleteId === peer.serverId ? (
                            <>
                              <Button
                                type="button"
                                variant="destructive"
                                size="sm"
                                disabled={peerActionBusy}
                                onClick={() => void deletePeer(peer.serverId)}
                              >
                                {intl.formatMessage({ id: "remoteDesktop.device.deleteConfirm" })}
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                disabled={peerActionBusy}
                                onClick={() => setPendingDeleteId(null)}
                              >
                                {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
                              </Button>
                            </>
                          ) : (
                            <>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                onClick={() => {
                                  setPeerActionError("");
                                  setPendingDeleteId(null);
                                  setEditingPeer({ serverId: peer.serverId, name });
                                }}
                              >
                                <Pencil className="size-4" />
                                {intl.formatMessage({ id: "remoteDesktop.device.rename" })}
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                onClick={() => {
                                  setPeerActionError("");
                                  setEditingPeer(null);
                                  setPendingDeleteId(peer.serverId);
                                }}
                              >
                                <Trash2 className="size-4" />
                                {intl.formatMessage({ id: "remoteDesktop.device.delete" })}
                              </Button>
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                disabled={!availability.canConnect}
                                onClick={() => connect(peer)}
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
                })}
              </ul>
            ) : null}
          </div>
        </div>
      </AutomationsMainBreadcrumbFrame>
    </div>
  );
}
