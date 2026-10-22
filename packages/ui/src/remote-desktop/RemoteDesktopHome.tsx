/**
 * 远程控制设备页（docs/specs/remote-control-unified-pairing.md）：
 * 一个本机配对码面板 + 一个设备列表。列表行 = LAN 已配对设备 / Mode Agent 设备 /
 * 发现的未配对 LAN 设备；三类设备同屏，动作按类型区分。
 *
 * 能力位不缓存：每次进入设备页与点「刷新」都重新探测（旧对端升级后立即生效）。
 * 已配对设备 / 已忽略的发现结果从检测列表过滤。
 */
import { useCallback, useEffect, useState } from "react";
import { LayoutGrid, List, MonitorSmartphone, Plus, RefreshCw, Settings2 } from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import type { CreateTaskRequest } from "@/app-shell/types.js";
import type { ModeAgentChatDevice } from "@/lib/workspaceSidePane.js";
import type { RemoteDesktopQuality } from "@mode/shared/remote-desktop";
import { DEFAULT_REMOTE_DESKTOP_QUALITY } from "@mode/shared/remote-desktop";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { AutomationsMainBreadcrumbFrame } from "@/settings/AutomationsMainBreadcrumbFrame.js";
import { logger } from "@/logger.js";
import type { RemoteDesktopDeviceProbe } from "@/remote-desktop/deviceAvailability.js";
import { filterUnpairedDiscoveredPeers } from "@/remote-desktop/deviceAvailability.js";
import { RemoteControlDeviceList } from "@/remote-desktop/RemoteControlDeviceList.js";
import { RemoteControlPairingCodePanel } from "@/remote-desktop/RemoteControlPairingCodePanel.js";
import { RemoteDesktopSession } from "@/remote-desktop/RemoteDesktopSession.js";
import { useDeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";
import { useDiscoveredLanPeers } from "@/remote-desktop/useDiscoveredLanPeers.js";
import { useModeAgentDevices } from "@/remote-desktop/useModeAgentDevices.js";
import { buildRemoteDevices } from "@/remote-desktop/remoteDeviceModel.js";

type PeerListState =
  | { status: "loading" }
  | { status: "ok"; peers: LanPairedPeer[] }
  | { status: "error" };

export function RemoteDesktopHome({
  isDesktop,
  showWindowControls = false,
  active,
  topOverlayClearancePaddingClass,
  onCreateTask,
  onOpenModeAgentChat,
  onConnectWorkspace,
  isRemoteWorkspace = false,
}: {
  isDesktop: boolean;
  isRemoteWorkspace?: boolean;
  onCreateTask?: (request?: CreateTaskRequest) => void;
  onOpenModeAgentChat?: (request: { device: ModeAgentChatDevice }) => void;
  /** 统一设备入口：LAN 设备行“连接工作区”直接把目标交给远程连接向导。 */
  onConnectWorkspace?: (target: LanPairedPeer) => void;
  /** 本页不渲染 WorkspaceHeader，窗控由顶部拖拽区右侧自绘（Windows/Linux）。 */
  showWindowControls?: boolean;
  /** 视图是否当前可见：分支常驻隐藏时为 false（spec UI 规则「切换保持连接」）。 */
  active: boolean;
  /** 左侧栏收起时让出顶部浮层左上入口宽度的 ps-* 类（仅会话面操作栏消费）。 */
  topOverlayClearancePaddingClass?: string | null;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const { viewMode, setViewMode } = useDeviceViewMode();
  // 与其它「跳设置分区」入口同款：先写入待消费分区意图，再由 tab store 打开设置 tab。
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [peers, setPeers] = useState<PeerListState>({ status: "loading" });
  const [probes, setProbes] = useState<Record<string, RemoteDesktopDeviceProbe>>({});
  const [locallyControlled, setLocallyControlled] = useState(false);
  const [sessionTarget, setSessionTarget] = useState<{
    serverId: string;
    name: string;
  } | null>(null);
  // 画质档位（P1-3）：选择入口在会话面顶部操作栏，状态记忆在页级（跨连接保持本次运行的
  // 上次选择）；连接时以该档下发，会话中改档由会话面走「bye + P2-1 重连」重建。
  const [quality, setQuality] = useState<RemoteDesktopQuality>(DEFAULT_REMOTE_DESKTOP_QUALITY);
  // 设备管理操作（重命名/删除）跨行互斥；错误与忙碌态上提到页级，行组件只报事件。
  const [peerActionBusy, setPeerActionBusy] = useState(false);
  const [peerActionError, setPeerActionError] = useState("");
  const {
    records: modeAgentRecords,
    refresh: refreshModeAgents,
    refreshStatus: refreshModeAgentStatus,
    startChat: startModeAgentChat,
    rename: renameModeAgent,
    remove: removeModeAgent,
  } = useModeAgentDevices({ isRemoteWorkspace, onCreateTask, onOpenModeAgentChat });

  /**
   * 单设备能力位探测（进页批量探测与行内「重新检测」共用一条写路径）：
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
      await Promise.all(list.map((peer) => probePeer(peer.serverId)));
    } catch (error) {
      logger.warn(
        "[remote-desktop] 读取已配对设备失败:",
        error instanceof Error ? error.message : String(error),
      );
      setPeers({ status: "error" });
    }
  }, [platform, probePeer]);

  const discovered = useDiscoveredLanPeers(loadPeers);
  const refreshAll = useCallback(async () => {
    await Promise.all([loadPeers(), refreshModeAgents()]);
  }, [loadPeers, refreshModeAgents]);

  /** 行内「重新检测」：只重探该设备，不动列表与其它设备的探测结果。 */
  const reprobePeer = useCallback(
    (serverId: string) => {
      void probePeer(serverId);
    },
    [probePeer],
  );

  // 首次挂载即在视图内；之后只在「重新可见」或「会话结束回到设备列表」时刷新探测——
  // 分支常驻隐藏期间不发网络请求。会话进行中（sessionTarget 非空）不刷新。
  useEffect(() => {
    if (active && !sessionTarget) {
      void loadPeers();
      void refreshModeAgents();
    }
  }, [active, sessionTarget, loadPeers, refreshModeAgents]);

  const openLanAccessSettings = useCallback(() => {
    // 合并后统一指向「远程控制」设置分区（被控 + 本机共享同一分区）。
    setPendingSettingsSectionIntent("remoteDesktop");
    openSettingsTab();
  }, [openSettingsTab]);

  // 发起会话由会话面自己完成（先订阅 `rd/session-event` 再 rdStartSession）：
  // main 在「WS 建立 → connected」推送后需要 renderer 立刻回 hello，设备页先起会话
  // 再挂载会话面会丢掉这条推送，握手就停在等 monitors 上。
  const connect = useCallback((peer: LanPairedPeer) => {
    setSessionTarget({
      serverId: peer.serverId,
      name: peer.name?.trim() || peer.serverId,
    });
  }, []);

  const connectWorkspace = useCallback(
    (peer: LanPairedPeer) => {
      onConnectWorkspace?.(peer);
    },
    [onConnectWorkspace],
  );

  const exitSession = useCallback(() => {
    // 回设备列表的刷新由上方 active/sessionTarget effect 统一触发（含本机被控状态重读）。
    setSessionTarget(null);
  }, []);

  /** 重命名（行内编辑提交）：只改本机记录的名称；成功后刷新列表。抛错由行转 onError。 */
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

  /** 删除（行内两段确认后执行）：清配对令牌与元数据，成功后刷新列表。 */
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

  if (sessionTarget) {
    return (
      <RemoteDesktopSession
        serverId={sessionTarget.serverId}
        deviceName={sessionTarget.name}
        quality={quality}
        onQualityChange={setQuality}
        topOverlayClearancePaddingClass={topOverlayClearancePaddingClass}
        onExit={exitSession}
      />
    );
  }

  const peerList = peers.status === "ok" ? peers.peers : [];
  const pairedServerIds = peerList.map((peer) => peer.serverId);
  const { visible: visibleDiscovered, hiddenCount } = filterUnpairedDiscoveredPeers(
    discovered.peers,
    pairedServerIds,
  );
  const devices = buildRemoteDevices({
    lanPeers: peerList,
    probes,
    modeAgents: modeAgentRecords,
    discovered: visibleDiscovered,
  });
  const hasDevices = devices.length > 0;

  // 设备页与自动化/插件市场同款：面包屑/桌面拖拽顶栏 + 共享标题排版。
  // 会话面（上方 sessionTarget 分支）保持整块无外框，视频占满主视图。
  return (
    <div className="flex h-full min-h-0 flex-col">
      <AutomationsMainBreadcrumbFrame
        isDesktop={isDesktop}
        showWindowControls={showWindowControls}
        sectionLabel={intl.formatMessage({ id: "remoteDesktop.title" })}
        ariaLabel={intl.formatMessage({ id: "remoteDesktop.breadcrumbLabel" })}
      >
        <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
          <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 px-4 py-4 md:px-6 md:py-6">
            <header className="space-y-5">
              <h1 className="text-2xl font-semibold tracking-tight text-foreground lg:text-3xl">
                {intl.formatMessage({ id: "remoteDesktop.title" })}
              </h1>
              <div className="flex flex-wrap items-start justify-between gap-3">
                <p className="min-w-0 flex-1 text-ui-base leading-6 text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.description" })}
                </p>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    type="button"
                    variant={viewMode === "list" ? "default" : "outline"}
                    size="icon"
                    data-testid="remote-control-view-list"
                    title={intl.formatMessage({ id: "remoteDesktop.devices.view.list" })}
                    aria-label={intl.formatMessage({ id: "remoteDesktop.devices.view.list" })}
                    onClick={() => setViewMode("list")}
                  >
                    <List className="size-4" aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant={viewMode === "grid" ? "default" : "outline"}
                    size="icon"
                    data-testid="remote-control-view-grid"
                    title={intl.formatMessage({ id: "remoteDesktop.devices.view.grid" })}
                    aria-label={intl.formatMessage({ id: "remoteDesktop.devices.view.grid" })}
                    onClick={() => setViewMode("grid")}
                  >
                    <LayoutGrid className="size-4" aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    data-testid="remote-control-refresh"
                    title={intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
                    aria-label={intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
                    disabled={peers.status === "loading" || discovered.scanning}
                    onClick={() => void refreshAll()}
                  >
                    <RefreshCw className="size-4" aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant="default"
                    data-testid="remote-control-scan"
                    disabled={discovered.scanning}
                    onClick={() => void discovered.scan()}
                  >
                    <Plus className="size-4" aria-hidden="true" />
                    {intl.formatMessage({
                      id: discovered.scanning
                        ? "remoteDesktop.devices.scanning"
                        : "remoteDesktop.devices.scan",
                    })}
                  </Button>
                </div>
              </div>
            </header>

            {/* 公网地址由内置中继随配对响应自动下发，页面不再提供手填入口。 */}
            {isDesktop && !isRemoteWorkspace ? (
              <RemoteControlPairingCodePanel active={active} />
            ) : null}

            {peerActionError ? (
              <p className="rounded-lg border border-border bg-card px-3 py-2 text-ui-base text-destructive">
                {peerActionError}
              </p>
            ) : null}

            {/* 本机被控中：整页置灰的根因横幅（逐行还有同样的原因文案）。 */}
            {locallyControlled ? (
              <p
                data-testid="remote-desktop-locally-controlled"
                className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-ui-base text-warning"
              >
                {intl.formatMessage({ id: "remoteDesktop.devices.locallyControlledBanner" })}
              </p>
            ) : null}

            {discovered.scanError ? (
              <p className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-ui-base text-warning">
                {discovered.scanError}
              </p>
            ) : null}

            {peers.status === "loading" && !hasDevices ? (
              <p className="rounded-lg border border-border bg-card px-3 py-2 text-ui-base text-foreground-subtle">
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
                  onClick={() => void refreshAll()}
                >
                  {intl.formatMessage({ id: "remoteDesktop.devices.retry" })}
                </Button>
              </div>
            ) : null}

            {peers.status === "ok" && !hasDevices && !discovered.scanning ? (
              <div
                data-testid="remote-control-empty"
                className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border bg-surface px-4 py-10 text-center"
              >
                <MonitorSmartphone className="size-6 text-foreground-subtlest" />
                <p className="max-w-md text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.devices.emptyAll" })}
                </p>
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void discovered.scan()}
                  >
                    <Plus className="size-4" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.scan" })}
                  </Button>
                  <Button type="button" variant="outline" size="sm" onClick={openLanAccessSettings}>
                    <Settings2 className="size-4" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.openLanAccess" })}
                  </Button>
                </div>
              </div>
            ) : null}

            {hasDevices ? (
              <RemoteControlDeviceList
                devices={devices}
                viewMode={viewMode}
                locallyControlled={locallyControlled}
                busy={peerActionBusy}
                onConnect={connect}
                onConnectWorkspace={onConnectWorkspace ? connectWorkspace : undefined}
                onRename={renamePeer}
                onDelete={deletePeer}
                onReprobe={reprobePeer}
                onStartChat={startModeAgentChat}
                onRefreshModeAgent={refreshModeAgentStatus}
                onRenameModeAgent={renameModeAgent}
                onRemoveModeAgent={removeModeAgent}
                onPairDiscovered={discovered.pair}
                onDismissDiscovered={discovered.dismiss}
                onError={setPeerActionError}
              />
            ) : null}

            {hiddenCount > 0 ? (
              <p className="text-ui-xs text-foreground-subtlest">
                {intl.formatMessage(
                  { id: "remoteDesktop.addDevice.pairedHidden" },
                  { count: String(hiddenCount) },
                )}
              </p>
            ) : null}
            {discovered.scanning ? (
              <p className="text-ui-xs text-foreground-subtlest">
                {intl.formatMessage({ id: "remoteDesktop.devices.scanning" })}
              </p>
            ) : null}
          </div>
        </div>
      </AutomationsMainBreadcrumbFrame>
    </div>
  );
}
