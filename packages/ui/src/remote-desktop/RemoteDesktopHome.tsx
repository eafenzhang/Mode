/**
 * 远程桌面设备页（docs/specs/lan-remote-desktop.md UI 规则「主视图 · 设备页」）：
 * 列出既有 LAN 配对设备（`getLanPairedPeers`），逐设备拉能力位（`rdGetPeerCapability`）；
 * 能力位缺失/离线/旧对端缺键 → 入口置灰并给出稳定文案；未配对 → 空态引导跳「设置 · 远程控制」。
 * 单卡渲染与卡内交互拆在 RemoteDesktopDeviceCard（max-lines 约束，2026-10-13）。
 *
 * 能力位不缓存：每次进入设备页与点「刷新」都重新探测（旧对端升级后立即生效）。
 * 「检测设备」列表过滤已配对设备（`filterUnpairedDiscoveredPeers`）。
 */
import { useCallback, useEffect, useState } from "react";
import { MonitorSmartphone, RefreshCw, Settings2 } from "lucide-react";
import type { LanPairedPeer } from "@mode/shared";
import type { RemoteDesktopQuality } from "@mode/shared/remote-desktop";
import { DEFAULT_REMOTE_DESKTOP_QUALITY } from "@mode/shared/remote-desktop";
import { Button } from "@/components/ui/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { AutomationsMainBreadcrumbFrame } from "@/settings/AutomationsMainBreadcrumbFrame.js";
import { logger } from "@/logger.js";
import type { RemoteDesktopDeviceProbe } from "@/remote-desktop/deviceAvailability.js";
import { RemoteDesktopAddDevice } from "@/remote-desktop/RemoteDesktopAddDevice.js";
import { RemoteDesktopDeviceCard } from "@/remote-desktop/RemoteDesktopDeviceCard.js";
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
  const [sessionTarget, setSessionTarget] = useState<{
    serverId: string;
    name: string;
  } | null>(null);
  // 画质档位（P1-3）：选择入口在会话面顶部操作栏，状态记忆在页级（跨连接保持本次运行的
  // 上次选择）；连接时以该档下发，会话中改档由会话面走「bye + P2-1 重连」重建。
  // （spec: lan-remote-desktop-native-media.md「画质档位」2026-10-13 入口迁移。）
  const [quality, setQuality] = useState<RemoteDesktopQuality>(DEFAULT_REMOTE_DESKTOP_QUALITY);
  // 设备管理操作（重命名/删除）跨卡互斥；错误与忙碌态上提到页级，卡片只报事件。
  const [peerActionBusy, setPeerActionBusy] = useState(false);
  const [peerActionError, setPeerActionError] = useState("");

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

  /** 卡片「重新检测」：只重探该设备，不动列表与其它设备的探测结果。 */
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
    }
  }, [active, sessionTarget, loadPeers]);

  const openLanAccessSettings = useCallback(() => {
    // 合并后统一指向「远程控制」设置分区（被控 + 本机共享同一分区）。
    setPendingSettingsSectionIntent("remoteDesktop");
    openSettingsTab();
  }, [openSettingsTab]);

  // 发起会话由会话面自己完成（先订阅 `rd/session-event` 再 rdStartSession）：
  // main 在「WS 建立 → connected」推送后需要 renderer 立刻回 hello，设备页先起会话
  // 再挂载会话面会丢掉这条推送，握手就停在等 monitors 上。
  // 画质档位由会话面操作栏选择（页级记忆），此处只带 serverId/name 建目标。
  const connect = useCallback((peer: LanPairedPeer) => {
    setSessionTarget({
      serverId: peer.serverId,
      name: peer.name?.trim() || peer.serverId,
    });
  }, []);

  const exitSession = useCallback(() => {
    // 回设备列表的刷新由上方 active/sessionTarget effect 统一触发（含本机被控状态重读）。
    setSessionTarget(null);
  }, []);

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

            {/* 本机被控中：整页置灰的根因横幅（逐卡还有同样的原因文案）。 */}
            {locallyControlled ? (
              <p
                data-testid="remote-desktop-locally-controlled"
                className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-ui-base text-warning"
              >
                {intl.formatMessage({ id: "remoteDesktop.devices.locallyControlledBanner" })}
              </p>
            ) : null}

            {/* 「检测设备 + 配对」在本页完成（从远程连接向导前移）；配对成功刷新设备列表。 */}
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
                    locallyControlled={locallyControlled}
                    busy={peerActionBusy}
                    onRename={renamePeer}
                    onDelete={deletePeer}
                    onReprobe={reprobePeer}
                    onConnect={connect}
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
