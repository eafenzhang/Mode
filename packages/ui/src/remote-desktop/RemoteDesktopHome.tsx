/**
 * 远程控制设备页（docs/specs/remote-control-unified-pairing.md R2）：
 * 只展示同账号 v3 Host。数据唯一来源是账号目录（`remoteV3Service.listHosts()`），
 * 页面不持有设备表、不探测能力位、不出现配对码/扫描发现/Mode Agent/LAN 已配对设备。
 *
 * 状态四态：未登录 / 加载中 / 已加载（含空目录）/ 加载失败；15s 轮询只在页面可见
 * 且不在会话中时进行（目录没有推送通道）。
 */
import { useCallback, useEffect, useState } from "react";
import { LayoutGrid, List, LogIn, MonitorSmartphone, RefreshCw, Settings2 } from "lucide-react";
import type { RemoteDesktopQuality } from "@mode/shared/remote-desktop";
import { DEFAULT_REMOTE_DESKTOP_QUALITY } from "@mode/shared/remote-desktop";
import type { RemoteV3DeviceSummary } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { openModeAccountDialog } from "@/lib/accountDialogNavigation.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useServices } from "@/hooks/useServices.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { AutomationsMainBreadcrumbFrame } from "@/settings/AutomationsMainBreadcrumbFrame.js";
import { logger } from "@/logger.js";
import { RemoteControlDeviceList } from "@/remote-desktop/RemoteControlDeviceList.js";
import { RemoteDesktopSession } from "@/remote-desktop/RemoteDesktopSession.js";
import { useDeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";
import { sortV3Hosts } from "@/remote-desktop/remoteDeviceModel.js";

type HostsState =
  | { status: "unconfigured" }
  | { status: "loading" }
  | { status: "ok"; hosts: RemoteV3DeviceSummary[] }
  | { status: "error"; message: string };

const DIRECTORY_POLL_MS = 15_000;

export function RemoteDesktopHome({
  isDesktop,
  showWindowControls = false,
  active,
  topOverlayClearancePaddingClass,
}: {
  isDesktop: boolean;
  /** 本页不渲染 WorkspaceHeader，窗控由顶部拖拽区右侧自绘（Windows/Linux）。 */
  showWindowControls?: boolean;
  /** 视图是否当前可见：分支常驻隐藏时为 false（spec UI 规则「切换保持连接」）。 */
  active: boolean;
  /** 左侧栏收起时让出顶部浮层左上入口宽度的 ps-* 类（仅会话面操作栏消费）。 */
  topOverlayClearancePaddingClass?: string | null;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const services = useServices();
  const baseServices = useOptionalBaseWorkspaceServices();
  const v3Service = baseServices?.remoteV3Service ?? services.remoteV3Service;
  const { viewMode, setViewMode } = useDeviceViewMode();
  // 与其它「跳设置分区」入口同款：先写入待消费分区意图，再由 tab store 打开设置 tab。
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [hostsState, setHostsState] = useState<HostsState>({ status: "loading" });
  const [locallyControlled, setLocallyControlled] = useState(false);
  const [sessionTarget, setSessionTarget] = useState<{
    deviceId: string;
    name: string;
  } | null>(null);
  // 画质档位：选择入口在会话面顶部操作栏，状态记忆在页级（跨连接保持本次运行的上次选择）。
  const [quality, setQuality] = useState<RemoteDesktopQuality>(DEFAULT_REMOTE_DESKTOP_QUALITY);

  /**
   * 目录读取。`silent` 用于 15s 轮询：失败时保留上一次成功列表，避免网络抖动把
   * 已有设备清空；首次进入失败则显式进入错误态并提供重试。
   */
  const loadHosts = useCallback(
    async (mode: "initial" | "silent" = "initial") => {
      if (!v3Service) {
        setHostsState({ status: "unconfigured" });
        return;
      }
      if (mode === "initial") {
        setHostsState((previous) => (previous.status === "ok" ? previous : { status: "loading" }));
      }
      try {
        const state = await v3Service.getState();
        if (!state.serverUrl || !state.deviceId) {
          setHostsState({ status: "unconfigured" });
          return;
        }
        setHostsState({ status: "ok", hosts: sortV3Hosts(await v3Service.listHosts()) });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        logger.warn(`[remote-desktop] 读取账号设备目录失败: ${message}`);
        if (mode === "silent") {
          setHostsState((previous) =>
            previous.status === "ok" ? previous : { status: "error", message },
          );
          return;
        }
        setHostsState({ status: "error", message });
      }
    },
    [v3Service],
  );

  const readControlledState = useCallback(() => {
    // 读失败按未被控展示；发起瞬间仍由 main 的 rd/start-session 权威拦截（locally_controlled）。
    void platform
      .rdGetControlledState()
      .then(({ controlled }) => setLocallyControlled(controlled))
      .catch(() => undefined);
  }, [platform]);

  useEffect(() => {
    if (active && !sessionTarget) {
      void loadHosts();
      readControlledState();
    }
  }, [active, sessionTarget, loadHosts, readControlledState]);

  useEffect(() => {
    if (!active || sessionTarget) return;
    const timer = setInterval(() => {
      void loadHosts("silent");
    }, DIRECTORY_POLL_MS);
    return () => clearInterval(timer);
  }, [active, sessionTarget, loadHosts]);

  const openLanAccessSettings = useCallback(() => {
    setPendingSettingsSectionIntent("remoteDesktop");
    openSettingsTab();
  }, [openSettingsTab]);

  const connect = useCallback((host: RemoteV3DeviceSummary) => {
    setSessionTarget({ deviceId: host.deviceId, name: host.name });
  }, []);

  const exitSession = useCallback(() => {
    setSessionTarget(null);
    // 本地断开后，被控端还要经历 bye → WS close → SessionClosed 的异步收口；
    // 首次重读可能早于它，补一拍避免「本机正在被远程控制」挂到手动刷新。
    setTimeout(readControlledState, 1000);
  }, [readControlledState]);

  if (sessionTarget) {
    return (
      <RemoteDesktopSession
        serverId={sessionTarget.deviceId}
        deviceName={sessionTarget.name}
        quality={quality}
        onQualityChange={setQuality}
        topOverlayClearancePaddingClass={topOverlayClearancePaddingClass}
        onExit={exitSession}
      />
    );
  }

  const hosts = hostsState.status === "ok" ? hostsState.hosts : [];
  const refreshing = hostsState.status === "loading";

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
                    disabled={refreshing || hostsState.status === "unconfigured"}
                    onClick={() => void loadHosts()}
                  >
                    <RefreshCw className="size-4" aria-hidden="true" />
                  </Button>
                </div>
              </div>
            </header>

            {/* 本机被控中：整页置灰的根因横幅（行内按钮也有同样的原因文案）。 */}
            {locallyControlled ? (
              <p
                data-testid="remote-desktop-locally-controlled"
                className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-ui-base text-warning"
              >
                {intl.formatMessage({ id: "remoteDesktop.devices.locallyControlledBanner" })}
              </p>
            ) : null}

            {hostsState.status === "loading" ? (
              <p className="rounded-lg border border-border bg-card px-3 py-2 text-ui-base text-foreground-subtle">
                {intl.formatMessage({ id: "remoteDesktop.devices.loading" })}
              </p>
            ) : null}

            {hostsState.status === "unconfigured" ? (
              <div
                data-testid="remote-control-signed-out"
                className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border bg-surface px-4 py-10 text-center"
              >
                <MonitorSmartphone className="size-6 text-foreground-subtlest" aria-hidden="true" />
                <div className="space-y-1">
                  <p className="text-ui-base font-medium text-foreground">
                    {intl.formatMessage({ id: "remoteDesktop.devices.signedOut" })}
                  </p>
                  <p className="max-w-md text-ui-base text-foreground-subtle">
                    {intl.formatMessage({ id: "remoteDesktop.devices.signedOutHint" })}
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <Button type="button" size="sm" onClick={openModeAccountDialog}>
                    <LogIn className="size-4" aria-hidden="true" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.login" })}
                  </Button>
                  <Button type="button" variant="outline" size="sm" onClick={openLanAccessSettings}>
                    <Settings2 className="size-4" aria-hidden="true" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.openLanAccess" })}
                  </Button>
                </div>
              </div>
            ) : null}

            {hostsState.status === "error" ? (
              <div
                data-testid="remote-control-load-error"
                className="flex flex-col items-center gap-3 rounded-lg border border-border bg-card px-4 py-8 text-center"
              >
                <p className="text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "remoteDesktop.devices.loadFailed" })}
                </p>
                <p className="max-w-md break-all text-ui-xs text-foreground-subtlest">
                  {hostsState.message}
                </p>
                <Button type="button" variant="outline" size="sm" onClick={() => void loadHosts()}>
                  {intl.formatMessage({ id: "remoteDesktop.devices.retry" })}
                </Button>
              </div>
            ) : null}

            {hostsState.status === "ok" && hosts.length === 0 ? (
              <div
                data-testid="remote-control-empty"
                className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border bg-surface px-4 py-10 text-center"
              >
                <MonitorSmartphone className="size-6 text-foreground-subtlest" aria-hidden="true" />
                <div className="space-y-1">
                  <p className="text-ui-base font-medium text-foreground">
                    {intl.formatMessage({ id: "remoteDesktop.devices.emptyNoHost" })}
                  </p>
                  <p className="max-w-md text-ui-base text-foreground-subtle">
                    {intl.formatMessage({ id: "remoteDesktop.devices.emptyNoHostHint" })}
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={openLanAccessSettings}>
                    <Settings2 className="size-4" aria-hidden="true" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.openLanAccess" })}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void loadHosts()}
                  >
                    <RefreshCw className="size-4" aria-hidden="true" />
                    {intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
                  </Button>
                </div>
              </div>
            ) : null}

            {hosts.length > 0 ? (
              <RemoteControlDeviceList
                hosts={hosts}
                viewMode={viewMode}
                locallyControlled={locallyControlled}
                onConnect={connect}
              />
            ) : null}
          </div>
        </div>
      </AutomationsMainBreadcrumbFrame>
    </div>
  );
}
