/**
 * 远程桌面设备页（docs/specs/remote-desktop-system-rdp.md「事件顺序」①②）：
 * 列出既有 LAN 配对设备（`getLanPairedPeers`），逐设备拉能力位与系统 RDP 状态
 * （`rdGetPeerCapability`）；不可用时入口置灰并给出稳定文案；未配对 → 空态引导跳「设置 · 远程控制」。
 *
 * 本页是「启动器 + 会话跟踪视图」：连接 = `rdConnect` 让 main 生成临时 `.rdp` 并启动 mstsc，
 * 画面/输入/剪贴板/音频全在外部窗口，应用**不渲染任何会话面**；会话列表只是 `rdGetSessions`
 * 的只读投影。单卡渲染与卡内交互拆在 RemoteDesktopDeviceCard（max-lines 约束）。
 *
 * 能力位不缓存：每次进入设备页与点「刷新」都重新探测（对端开启系统 RDP 后立即生效）。
 */
import { useCallback, useEffect, useState } from "react";
import { MonitorSmartphone, RefreshCw, Settings2 } from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import type { RdpConnectionOptions } from "@mode/shared/remote-desktop-rdp";
import { Button } from "@/components/ui/button.js";
import { toast } from "@/components/ui/toast.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { AutomationsMainBreadcrumbFrame } from "@/settings/AutomationsMainBreadcrumbFrame.js";
import { logger } from "@/logger.js";
import type { RemoteDesktopDeviceProbe } from "@/remote-desktop/deviceAvailability.js";
import type { RemoteDesktopSessionRecord } from "@/remote-desktop/platformTypes.js";
import { RemoteDesktopAddDevice } from "@/remote-desktop/RemoteDesktopAddDevice.js";
import { RemoteDesktopDeviceCard } from "@/remote-desktop/RemoteDesktopDeviceCard.js";
import { RemoteDesktopSessionList } from "@/remote-desktop/RemoteDesktopSessionList.js";

type PeerListState =
  | { status: "loading" }
  | { status: "ok"; peers: LanPairedPeer[] }
  | { status: "error" };

export function RemoteDesktopHome({
  isDesktop,
  active,
}: {
  isDesktop: boolean;
  /** 视图是否当前可见：分支常驻隐藏时为 false（隐藏期间不发网络请求）。 */
  active: boolean;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  // 与其它「跳设置分区」入口同款：先写入待消费分区意图，再由 tab store 打开设置 tab。
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [peers, setPeers] = useState<PeerListState>({ status: "loading" });
  const [probes, setProbes] = useState<Record<string, RemoteDesktopDeviceProbe>>({});
  // 已连接会话：唯一来源是主控 main 的会话表（`rdGetSessions`），本页只做只读投影。
  const [sessions, setSessions] = useState<RemoteDesktopSessionRecord[]>([]);
  // 设备管理/连接操作跨卡互斥；错误与忙碌态上提到页级，卡片只报事件。
  const [peerActionBusy, setPeerActionBusy] = useState(false);
  const [peerActionError, setPeerActionError] = useState("");
  const [connectingServerId, setConnectingServerId] = useState<string | null>(null);

  /**
   * 单设备能力位探测（进页批量探测与卡片「重新检测」共用一条写路径）：
   * 先置 probing 再覆盖结果，失败按 probe-failed（能力位不猜测）。
   */
  const probePeer = useCallback(
    async (serverId: string) => {
      setProbes((previous) => ({ ...previous, [serverId]: { status: "probing" } }));
      try {
        const capability = await platform.rdGetPeerCapability(serverId);
        setProbes((previous) => ({ ...previous, [serverId]: { status: "ok", capability } }));
      } catch (error) {
        logger.warn(
          `[remote-desktop] 设备能力位探测失败 serverId=${serverId}:`,
          error instanceof Error ? error.message : String(error),
        );
        setProbes((previous) => ({ ...previous, [serverId]: { status: "probe-failed" } }));
      }
    },
    [platform],
  );

  /** 刷新会话列表（读失败按空列表展示，不阻塞设备列表）。 */
  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await platform.rdGetSessions());
    } catch (error) {
      logger.warn(
        "[remote-desktop] 读取会话列表失败:",
        error instanceof Error ? error.message : String(error),
      );
    }
  }, [platform]);

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
      await Promise.all(list.map((peer) => probePeer(peer.serverId)));
    } catch (error) {
      logger.warn(
        "[remote-desktop] 读取已配对设备失败:",
        error instanceof Error ? error.message : String(error),
      );
      setPeers({ status: "error" });
    }
  }, [platform, probePeer]);

  /** 卡片「重新检测」：只重探该设备，不动列表与其它设备的探测结果。 */
  const reprobePeer = useCallback(
    (serverId: string) => {
      void probePeer(serverId);
    },
    [probePeer],
  );

  // 首次挂载即在视图内；之后只在「重新可见」时刷新——分支常驻隐藏期间不发网络请求。
  useEffect(() => {
    if (active) {
      void loadPeers();
      void refreshSessions();
    }
  }, [active, loadPeers, refreshSessions]);

  const openLanAccessSettings = useCallback(() => {
    setPendingSettingsSectionIntent("remoteDesktop");
    openSettingsTab();
  }, [openSettingsTab]);

  /**
   * 发起连接：main 生成临时 `.rdp` 并启动 mstsc。
   * - 不带 options：用 main 里已存的每设备选项；
   * - 带 options（连接选项面板「确认」）：先持久化，再用回读值发起（单一写路径）。
   * 成功后只提示「已在外部窗口打开」并刷新会话列表——本页不渲染画面。
   */
  const connect = useCallback(
    async (peer: LanPairedPeer, options?: RdpConnectionOptions) => {
      setPeerActionBusy(true);
      setPeerActionError("");
      setConnectingServerId(peer.serverId);
      const name = peer.name?.trim() || peer.serverId;
      try {
        let effective = options;
        if (effective) {
          effective = await platform.rdSetRdpOptions(peer.serverId, effective);
        } else {
          effective = await platform.rdGetRdpOptions(peer.serverId);
        }
        const result = await platform.rdConnect({
          serverId: peer.serverId,
          name,
          address: peer.host,
          options: effective,
        });
        if (!result.ok) {
          setPeerActionError(
            intl.formatMessage(
              { id: `remoteDesktop.connect.failed.${result.reason}` },
              { name },
            ),
          );
          return;
        }
        toast(intl.formatMessage({ id: "remoteDesktop.connect.opened" }, { name }));
        await refreshSessions();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        logger.warn(`[remote-desktop] 发起连接失败 serverId=${peer.serverId}:`, message);
        setPeerActionError(message);
      } finally {
        setConnectingServerId(null);
        setPeerActionBusy(false);
      }
    },
    [intl, platform, refreshSessions],
  );

  /** 断开某条会话：终止对应 mstsc 进程，成功后刷新列表。 */
  const disconnect = useCallback(
    async (sessionId: string) => {
      setPeerActionBusy(true);
      setPeerActionError("");
      try {
        await platform.rdDisconnect(sessionId);
        await refreshSessions();
      } catch (error) {
        setPeerActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setPeerActionBusy(false);
      }
    },
    [platform, refreshSessions],
  );

  /** 重命名（卡片编辑态提交）：只改本机记录的名称；成功后刷新列表。抛错由卡片转 onError。 */
  const renamePeer = useCallback(
    async (serverId: string, name: string) => {
      setPeerActionBusy(true);
      setPeerActionError("");
      try {
        await platform.renameLanPairedPeer(serverId, name);
        await loadPeers();
      } finally {
        setPeerActionBusy(false);
      }
    },
    [loadPeers, platform],
  );

  /** 删除（卡片两段式确认后执行）：清配对令牌与元数据，成功后刷新列表。 */
  const deletePeer = useCallback(
    async (serverId: string) => {
      setPeerActionBusy(true);
      setPeerActionError("");
      try {
        await platform.removeLanPairedPeer(serverId);
        await loadPeers();
      } finally {
        setPeerActionBusy(false);
      }
    },
    [loadPeers, platform],
  );

  const peerList = peers.status === "ok" ? peers.peers : [];
  const pairedServerIds = peerList.map((peer) => peer.serverId);

  // 设备页与自动化/插件市场同款：面包屑/桌面拖拽顶栏 + 共享标题排版。
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
                onClick={() => {
                  void loadPeers();
                  void refreshSessions();
                }}
              >
                <RefreshCw className="size-4" />
                {intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
              </Button>
            </header>

            {peerActionError ? (
              <p
                data-testid="remote-desktop-action-error"
                className="rounded-lg border border-border bg-card px-3 py-2 text-ui-base text-destructive"
              >
                {peerActionError}
              </p>
            ) : null}

            {/* 已连接会话（外部 mstsc 窗口）的应用内跟踪视图。 */}
            <RemoteDesktopSessionList
              sessions={sessions}
              busy={peerActionBusy}
              onDisconnect={(sessionId) => void disconnect(sessionId)}
            />

            {/* 「检测设备 + 配对」在本页完成；配对成功刷新设备列表。 */}
            <RemoteDesktopAddDevice onPaired={loadPeers} pairedServerIds={pairedServerIds} />

            {peers.status === "loading" ? (
              <div className="grid gap-3 sm:grid-cols-2" data-testid="remote-desktop-loading">
                {[0, 1].map((key) => (
                  <div
                    key={key}
                    aria-hidden="true"
                    className="flex animate-pulse flex-col overflow-hidden rounded-xl border border-border bg-card"
                  >
                    <div className="flex flex-col gap-3 p-3">
                      <div className="flex items-center gap-2">
                        <div className="h-5 w-14 shrink-0 rounded-full bg-surface" />
                        <div className="h-3 w-1/2 rounded bg-surface" />
                      </div>
                      <div className="aspect-[16/9] w-full rounded-lg bg-surface" />
                    </div>
                    <div className="h-9 border-t border-border" />
                  </div>
                ))}
              </div>
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
                {peerList.map((peer) => (
                  <RemoteDesktopDeviceCard
                    key={peer.serverId}
                    peer={peer}
                    probe={probes[peer.serverId]}
                    busy={peerActionBusy}
                    connecting={connectingServerId === peer.serverId}
                    onRename={renamePeer}
                    onDelete={deletePeer}
                    onReprobe={reprobePeer}
                    onConnect={(target, options) => void connect(target, options)}
                    onError={setPeerActionError}
                  />
                ))}
              </ul>
            ) : null}
          </div>
        </div>
      </AutomationsMainBreadcrumbFrame>
    </div>
  );
}
