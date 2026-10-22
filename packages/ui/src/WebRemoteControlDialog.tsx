/* eslint-disable max-lines -- 远控弹层同时维护 LAN 快照、二维码/配对码与 Bot Channel 入口；拆分会打散同一弹层的状态生命周期。 */
import { memo, useEffect, useState } from "react";
import type { BotProvider, LanAccessState } from "@mode/shared";
import {
  Bot as BotIcon,
  Copy as CopyIcon,
  LoaderIcon,
  MonitorSmartphone,
  QrCode,
  RefreshCw,
  XIcon,
} from "lucide-react";
import QRCode from "qrcode";
import { BotsDialog } from "@/BotsDialog.js";
import { ProviderIcon } from "@/BotsDialog/shared.js";
import { cn } from "@/components/lib/utils.js";
import { Button } from "@/ui-foundation/adapters/button.js";
import { toast } from "@/ui-foundation/adapters/toast.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";
import { setPendingSettingsSectionIntent } from "@/lib/settingsNavigation.js";
import { resolveActiveWebSessions, resolveWebRemoteUrl } from "@/lib/webRemoteEntry.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import { getBotProviderRegionTagLabelId } from "@/botsUi.js";

function formatRemaining(expiresAt: number, now: number): string {
  const remainingSeconds = Math.max(0, Math.round((expiresAt - now) / 1000));
  const minutes = Math.floor(remainingSeconds / 60);
  const seconds = remainingSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

type RemoteControlBotProvider = Extract<BotProvider, "weixin" | "wecom" | "feishu" | "dingtalk">;

// 通道只保留微信 / 企业微信 / 飞书（中国域）/ 钉钉，且全部一次展示（不再折叠）。
const REMOTE_CONTROL_ENTRIES: Array<{
  provider: RemoteControlBotProvider;
}> = [
  { provider: "weixin" },
  { provider: "wecom" },
  { provider: "feishu" },
  { provider: "dingtalk" },
];

export const WebRemoteControlDialog = memo(function WebRemoteControlDialogComponent({
  open,
  onOpenChange,
  workspacePath,
  workspaceIdentity,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspacePath: string;
  workspaceIdentity?: string;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const baseServices = useOptionalBaseWorkspaceServices();
  const lanService = baseServices?.lanAccessService;
  const openSettingsTab = useOptionalTabStore((state) => state.openSettingsTab);
  const [botsDialogOpen, setBotsDialogOpen] = useState(false);
  const [botEntryProvider, setBotEntryProvider] = useState<RemoteControlBotProvider | null>(null);
  const [lanState, setLanState] = useState<LanAccessState | null>(null);
  const [lanLoading, setLanLoading] = useState(false);
  const [lanBusy, setLanBusy] = useState(false);
  const [lanError, setLanError] = useState("");
  const [lanQrDataUrl, setLanQrDataUrl] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  // 打开弹层时读一次 LAN 状态；状态所有者仍是 Host 的 lanAccessServer，这里只读快照。
  useEffect(() => {
    if (!open) {
      return;
    }
    let cancelled = false;
    setLanLoading(true);
    setLanError("");
    void platform
      .getLanAccessState()
      .then((state) => {
        if (!cancelled) {
          setLanState(state);
          setNow(Date.now());
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setLanState(null);
          setLanError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLanLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, platform]);

  // 在线会话数由 Host 的 /ws 连接统计随 onDidChange 推送；没有 base 服务（Web/SSR）时轮询兜底，
  // 否则弹层会永远停在「等待手机连接」。
  useEffect(() => {
    if (!open) {
      return;
    }
    let cancelled = false;
    const apply = (next: LanAccessState) => {
      if (!cancelled) {
        setLanState(next);
      }
    };
    if (lanService) {
      const subscription = lanService.onDidChange(apply);
      return () => {
        cancelled = true;
        subscription.dispose();
      };
    }
    const timer = setInterval(() => {
      void platform.getLanAccessState().then(apply).catch(() => undefined);
    }, 3_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [open, lanService, platform]);

  const webUrl = resolveWebRemoteUrl(lanState);
  const activeWebSessions = resolveActiveWebSessions(lanState);
  const webConnected = activeWebSessions > 0;

  // 二维码只做展示：地址来自 LAN 快照，不新增网络请求。
  useEffect(() => {
    if (!webUrl) {
      setLanQrDataUrl(null);
      return;
    }
    let cancelled = false;
    void QRCode.toDataURL(webUrl, { margin: 1, width: 160 })
      .then((dataUrl) => {
        if (!cancelled) {
          setLanQrDataUrl(dataUrl);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setLanQrDataUrl(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [webUrl]);

  useEffect(() => {
    if (!lanState?.pairCode) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [lanState?.pairCode]);

  const handleCreatePairCode = async () => {
    setLanBusy(true);
    setLanError("");
    try {
      const next = await platform.createLanAccessPairCode();
      setLanState(next);
      setNow(Date.now());
    } catch (error) {
      setLanError(
        `${intl.formatMessage({ id: "settings.lanAccess.pairCodeFailed" })}：${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    } finally {
      setLanBusy(false);
    }
  };

  const handleOpenLanSettings = () => {
    setPendingSettingsSectionIntent("remoteDesktop");
    openSettingsTab?.();
    onOpenChange(false);
  };

  const handleCopyLink = async () => {
    if (!webUrl) {
      return;
    }
    try {
      await navigator.clipboard.writeText(webUrl);
      toast(intl.formatMessage({ id: "webRemoteControl.qr.copied" }));
    } catch (error) {
      logger.warn("[WebRemoteControlDialog] 复制远控链接失败", error);
      toast(intl.formatMessage({ id: "webRemoteControl.qr.copyFailed" }));
    }
  };

  const handleOpenBotEntry = (provider: RemoteControlBotProvider) => {
    setBotEntryProvider(provider);
    setBotsDialogOpen(true);
    logger.info("[WebRemoteControlDialog] 打开 Bot Channel 配置入口", {
      workspacePath,
      workspaceIdentity: workspaceIdentity ?? "none",
      provider,
    });
  };

  const handleOpenBotsDialog = () => {
    setBotEntryProvider(null);
    setBotsDialogOpen(true);
    logger.info("[WebRemoteControlDialog] 打开 Bots 总配置入口", {
      workspacePath,
      workspaceIdentity: workspaceIdentity ?? "none",
    });
  };

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent
          showCloseButton={false}
          className="max-h-[calc(100vh-6rem)] max-w-4xl gap-0 overflow-hidden rounded-2xl p-0"
        >
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            // Bugfix: 这个弹窗会贴近桌面窗口顶部显示，默认 close 在 Electron drag 区里容易点不中。
            // 这里改成显式点击关闭，并把按钮本身标成 no-drag，保证右上角关闭动作能稳定命中。
            // Bugfix: 远控弹层内可点击控件之前没有显式 pointer cursor，桌面端 hover 时不像可操作元素。
            // 这里仅给启用态补手指指针，禁用态仍沿用 Button 的 disabled 交互语义。
            className="absolute top-2 end-2 enabled:cursor-pointer [app-region:no-drag]"
            onClick={() => onOpenChange(false)}
          >
            <XIcon />
            <span className="sr-only">Close</span>
          </Button>
          <div className="max-h-[calc(100vh-6rem)] min-h-0 overflow-y-auto p-5">
            <DialogHeader className="space-y-2 pe-8">
              <div className="flex items-center gap-2">
                <div className="flex size-10 items-center justify-center rounded-lg border border-border bg-surface text-primary">
                  <MonitorSmartphone className="size-5" />
                </div>
                <div className="space-y-1">
                  <DialogTitle>{intl.formatMessage({ id: "webRemoteControl.title" })}</DialogTitle>
                  <DialogDescription>
                    {intl.formatMessage({ id: "webRemoteControl.description" })}
                  </DialogDescription>
                </div>
              </div>
            </DialogHeader>

            <div className="mt-5 grid gap-4 md:grid-cols-2">
              <section className="flex min-h-[360px] flex-col rounded-xl border border-border bg-card p-4">
                <div className="flex items-start gap-2">
                  <MonitorSmartphone className="mt-0.5 size-4 shrink-0 text-foreground-subtle" />
                  <div className="min-w-0 space-y-1">
                    <div className="text-ui-base font-medium text-foreground">
                      {intl.formatMessage({ id: "webRemoteControl.qr.title" })}
                    </div>
                    <p className="text-ui-base/relaxed text-foreground-subtle">
                      {intl.formatMessage({ id: "webRemoteControl.qr.description" })}
                    </p>
                  </div>
                </div>
                {lanLoading ? (
                  <div className="mt-4 flex items-center gap-2 text-ui-base text-foreground-subtle">
                    <LoaderIcon className="size-4 animate-spin" />
                    {intl.formatMessage({ id: "webRemoteControl.web.loading" })}
                  </div>
                ) : lanState?.enabled ? (
                  webUrl ? (
                    <>
                      <div className="mt-4 flex items-center justify-between gap-2 rounded-lg border border-border bg-surface px-3 py-2">
                        <span className="flex items-center gap-2 text-ui-base text-foreground-subtle">
                          <span
                            className={cn(
                              "size-1.5 rounded-full",
                              webConnected ? "bg-success" : "bg-warning",
                            )}
                          />
                          {webConnected
                            ? intl.formatMessage(
                                { id: "webRemoteControl.qr.connected" },
                                { count: String(activeWebSessions) },
                              )
                            : intl.formatMessage({ id: "webRemoteControl.qr.waiting" })}
                          <span className="inline-flex h-5 items-center rounded-full border border-border px-2 text-ui-xs font-medium text-foreground-subtle">
                            {intl.formatMessage({
                              id: webConnected
                                ? "webRemoteControl.qr.online"
                                : "webRemoteControl.qr.ready",
                            })}
                          </span>
                        </span>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() => onOpenChange(false)}
                        >
                          {intl.formatMessage({ id: "webRemoteControl.qr.close" })}
                        </Button>
                      </div>
                      <p className="mt-3 text-ui-base/relaxed text-foreground-subtle">
                        {intl.formatMessage({ id: "webRemoteControl.qr.hint" })}
                      </p>
                      <div className="mt-3 flex min-h-0 flex-1 flex-col items-center justify-center rounded-xl border border-dashed border-border bg-surface p-4">
                        {lanQrDataUrl ? (
                          <img
                            src={lanQrDataUrl}
                            alt={intl.formatMessage({ id: "webRemoteControl.web.qrAlt" })}
                            className="size-52"
                          />
                        ) : (
                          <QrCode className="size-12 text-foreground-subtle" />
                        )}
                        <div className="mt-3 w-full space-y-1 text-center">
                          <div className="break-all font-mono text-ui-sm text-foreground">
                            {webUrl}
                          </div>
                          {lanState.pairCode ? (
                            <div className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-ui-xs text-foreground-subtle">
                              <span>
                                {intl.formatMessage({ id: "webRemoteControl.web.pairCode" })}：
                                <span className="font-mono text-ui-base tracking-[0.25em] text-foreground">
                                  {lanState.pairCode.code}
                                </span>
                              </span>
                              <span>
                                {intl.formatMessage(
                                  { id: "webRemoteControl.web.pairCodeExpires" },
                                  { time: formatRemaining(lanState.pairCode.expiresAt, now) },
                                )}
                              </span>
                            </div>
                          ) : (
                            <div className="text-ui-xs text-foreground-subtle">
                              {intl.formatMessage({ id: "webRemoteControl.qr.pairCodeHint" })}
                            </div>
                          )}
                        </div>
                      </div>
                      <p className="mt-3 text-ui-sm text-foreground-subtle">
                        {intl.formatMessage({ id: "webRemoteControl.qr.unableHint" })}
                      </p>
                      <div className="mt-3 flex gap-2">
                        <Button
                          type="button"
                          variant="outline"
                          size="lg"
                          className="flex-1 justify-center"
                          disabled={lanBusy}
                          onClick={() => void handleCreatePairCode()}
                        >
                          {lanBusy ? (
                            <LoaderIcon className="size-4 animate-spin" />
                          ) : (
                            <RefreshCw className="size-4" />
                          )}
                          {intl.formatMessage({ id: "webRemoteControl.qr.refresh" })}
                        </Button>
                        <Button
                          type="button"
                          variant="outline"
                          size="lg"
                          className="flex-1 justify-center"
                          onClick={() => void handleCopyLink()}
                        >
                          <CopyIcon className="size-4" />
                          {intl.formatMessage({ id: "webRemoteControl.qr.copyLink" })}
                        </Button>
                      </div>
                    </>
                  ) : (
                    <div className="mt-4 text-ui-base text-foreground-subtle">
                      {intl.formatMessage({ id: "webRemoteControl.web.unavailable" })}
                    </div>
                  )
                ) : (
                  <div className="mt-4 flex flex-1 flex-col items-center justify-center gap-3">
                    <span className="text-ui-base text-foreground-subtle">
                      {intl.formatMessage({ id: "webRemoteControl.web.disabled" })}
                    </span>
                    <Button
                      type="button"
                      variant="outline"
                      size="lg"
                      onClick={handleOpenLanSettings}
                    >
                      {intl.formatMessage({ id: "webRemoteControl.web.openSettings" })}
                    </Button>
                  </div>
                )}
                {lanError ? <p className="mt-3 text-ui-sm text-destructive">{lanError}</p> : null}
              </section>
              <section className="flex min-h-[360px] flex-col rounded-xl border border-border bg-card p-4">
                <div className="mb-4 flex items-start gap-2">
                  <BotIcon className="mt-0.5 size-4 shrink-0 text-foreground-subtle" />
                  <div className="min-w-0 space-y-1">
                    <div className="text-ui-base font-medium text-foreground">
                      {intl.formatMessage({
                        id: "webRemoteControl.botChannel.title",
                      })}
                    </div>
                    <p className="text-ui-base/relaxed text-foreground-subtle">
                      {intl.formatMessage({
                        id: "webRemoteControl.botChannel.description",
                      })}
                    </p>
                  </div>
                </div>
                <div className="grid min-h-0 flex-1 gap-3">
                  {REMOTE_CONTROL_ENTRIES.map((entry) => {
                    const regionTagLabelId = getBotProviderRegionTagLabelId(entry.provider);

                    return (
                      <button
                        key={entry.provider}
                        type="button"
                        className="flex min-h-0 cursor-pointer items-start gap-3 rounded-lg border border-transparent bg-surface px-3 py-3 text-start transition-colors hover:border-input-border-focused hover:bg-surface-hover focus-visible:border-input-border-focused"
                        onClick={() => handleOpenBotEntry(entry.provider)}
                      >
                        {/* Bugfix: 远控 Bot Channel 入口原来用通用 lucide 图标，用户无法一眼区分微信、飞书和 Telegram。
                            这里直接复用 BotsDialog 的渠道 logo，不再额外包裹容器，保证品牌图标本身作为视觉识别。 */}
                        <ProviderIcon provider={entry.provider} className="size-12 shrink-0" />
                        <span className="min-w-0 flex-1 space-y-1">
                          <span className="flex min-w-0 items-center gap-1.5 text-ui-base font-medium text-foreground">
                            <span className="min-w-0 truncate">
                              {intl.formatMessage({
                                id: `webRemoteControl.botChannel.${entry.provider}.title`,
                              })}
                            </span>
                            {regionTagLabelId ? (
                              <span className="inline-flex h-5 shrink-0 items-center rounded-full border border-border px-2 text-ui-xs font-medium leading-none text-foreground-subtle">
                                {intl.formatMessage({ id: regionTagLabelId })}
                              </span>
                            ) : null}
                          </span>
                          <span className="block text-ui-base/relaxed text-foreground-subtle">
                            {intl.formatMessage({
                              id: `webRemoteControl.botChannel.${entry.provider}.description`,
                            })}
                          </span>
                          <span className="block text-ui-base font-medium text-primary">
                            {intl.formatMessage({
                              id: "webRemoteControl.botChannel.configure",
                            })}
                          </span>
                        </span>
                      </button>
                    );
                  })}
                </div>
                <div className="mt-3">
                  <Button
                    type="button"
                    variant="outline"
                    size="lg"
                    className="w-full justify-center gap-2 enabled:cursor-pointer"
                    onClick={handleOpenBotsDialog}
                  >
                    <BotIcon className="size-3.5" />
                    {intl.formatMessage({
                      id: "webRemoteControl.botChannel.manageBots",
                    })}
                  </Button>
                </div>
              </section>
            </div>
          </div>
        </DialogContent>
      </Dialog>
      <BotsDialog
        open={botsDialogOpen}
        onOpenChange={setBotsDialogOpen}
        workspacePath={workspacePath}
        workspaceIdentity={workspaceIdentity}
        entryProvider={botEntryProvider}
      />
    </>
  );
});
