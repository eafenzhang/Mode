/* eslint-disable max-lines -- ProviderSettingsCard 集中维护 Bot 凭据、二维码和绑定状态机展示；后续拆分 provider 子组件后移除。 */
import {
  CircleAlert,
  Clock3,
  Copy,
  KeyRound,
  LoaderCircle,
  QrCode,
  Unlink,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import type { BotConfig, BotServiceStatus } from "@mode/shared";
import { isFeishuBotProvider } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";
import { cn } from "@/components/lib/utils.js";
import {
  botOnboardingStageDotClass,
  deriveBotOnboardingStages,
  type BotOnboardingStage,
} from "@/lib/botOnboardingStages.js";
import type {
  BindCodeState,
  FeishuRegistrationState,
  WecomRegistrationState,
  WeixinRegistrationState,
} from "./shared.js";
import { formatBindCountdown } from "./shared.js";

export function DetailPanel({ children }: { children: ReactNode }) {
  return <div className="rounded-lg bg-background p-3">{children}</div>;
}

/**
 * 接入三段状态行（spec docs/specs/im-bot-onboarding.md R3-4）。
 * 判定全部来自 `lib/botOnboardingStages` 的纯函数，这里只负责渲染：同一件事不再有多处说法。
 * 圆点配色沿用 shared.tsx `runtimeDot` 的词表（就绪绿 / 待办灰 / 异常红）。
 */
function BotOnboardingStageRow({ stages }: { stages: BotOnboardingStage[] }) {
  const { intl } = useModeIntl();
  return (
    <span className="inline-flex flex-wrap items-center justify-end gap-x-3 gap-y-1">
      {stages.map((stage) => (
        <span
          key={stage.id}
          className="inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap text-ui-base text-foreground-subtle"
        >
          <span className={cn("size-2 rounded-full", botOnboardingStageDotClass(stage.state))} />
          {intl.formatMessage({ id: `bots.onboarding.${stage.id}.${stage.state}` })}
        </span>
      ))}
    </span>
  );
}

/**
 * 飞书配置面板：两条并行的接入路径（spec R2「并存而非替换」）。
 * - 扫码：调用飞书扫码注册接口，确认后自动拿到 appId + appSecret；
 * - 手动：在飞书开放平台自建应用后，手动填写 App ID 与 App Secret。
 *
 * 手填这条路径是 2026-10-14 补回的：Bots 重构时先丢了手填、只剩扫码，两段历史各丢过一次
 * （`BotsDialog.tsx` 中扫码成功的注释即记载了「服务层一直支持扫码，UI 只保留了手填」）。
 * 服务层无需改动——`saveBot({ ...bot, feishuAppId }, { credentialValue: appSecret })` 走的就是
 * 与扫码成功完全相同的写入路径（R4：凭据只经 credentialService，无第二条写路径）。
 */
function FeishuSetupPanel({
  bot,
  registration,
  registrationLoading,
  credentialValue,
  secretSaving,
  onCredentialValueChange,
  onStartRegistration,
  onSaveManual,
}: {
  bot: BotConfig;
  registration: FeishuRegistrationState | null;
  registrationLoading: boolean;
  credentialValue: string;
  secretSaving: boolean;
  onCredentialValueChange: (value: string) => void;
  onStartRegistration: () => void;
  onSaveManual: (feishuAppId: string, secret: string) => void;
}) {
  const { intl } = useModeIntl();
  const [appIdDraft, setAppIdDraft] = useState(bot.feishuAppId ?? "");
  const active = registration?.botId === bot.id ? registration : null;
  const canSaveManual = Boolean(appIdDraft.trim() && credentialValue.trim()) && !secretSaving;

  return (
    <DetailPanel>
      <div className="space-y-4 text-ui-base text-foreground-subtle">
        <div className="flex flex-wrap items-start gap-4">
          {active?.qrDataUrl ? (
            <img
              src={active.qrDataUrl}
              alt={intl.formatMessage({ id: "bots.feishuRegistrationQrAlt" })}
              className="size-40 shrink-0 rounded-lg border border-border bg-surface p-2"
            />
          ) : (
            <div className="flex size-40 shrink-0 items-center justify-center rounded-lg border border-dashed border-border bg-surface">
              <QrCode className="size-8 text-foreground-subtle" />
            </div>
          )}
          <div className="min-w-52 flex-1 space-y-2">
            <div className="text-foreground">
              {intl.formatMessage({ id: "bots.feishuRegistrationTitle" })}
            </div>
            <div>{intl.formatMessage({ id: "bots.feishuRegistrationDescription" })}</div>
            {active ? (
              <>
                <div>
                  {intl.formatMessage({
                    id: `bots.feishuRegistrationScanHint.${active.domain}`,
                  })}
                </div>
                <div className="rounded-md bg-surface px-2 py-1 font-mono text-foreground">
                  {active.userCode}
                </div>
                <div>
                  {active.status === "pending" ? (
                    <span className="inline-flex items-center gap-1">
                      <LoaderCircle className="size-3 animate-spin" />
                      {intl.formatMessage({ id: "common.loading" })}
                    </span>
                  ) : (
                    (active.message ??
                    intl.formatMessage({
                      id: `bots.feishuRegistration.${active.status}`,
                    }))
                  )}
                </div>
              </>
            ) : null}
            <Button
              variant="outline"
              size="lg"
              onClick={onStartRegistration}
              disabled={registrationLoading}
            >
              {registrationLoading ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : (
                <QrCode className="size-4" />
              )}
              {intl.formatMessage({ id: "bots.feishuRegistrationButton" })}
            </Button>
          </div>
        </div>

        <div className="border-t border-border pt-3">
          <div className="text-foreground">
            {intl.formatMessage({ id: "bots.feishuManualTitle" })}
          </div>
          <div className="mt-1">{intl.formatMessage({ id: "bots.feishuManualDescription" })}</div>
          <div className="mt-2 grid gap-2">
            <Input
              size="lg"
              value={appIdDraft}
              onChange={(event) => setAppIdDraft(event.target.value)}
              placeholder={intl.formatMessage({ id: "bots.feishuAppIdPlaceholder" })}
              disabled={secretSaving}
            />
            <div className="flex w-full min-w-0 items-center gap-2">
              <Input
                size="lg"
                type="password"
                value={credentialValue}
                onChange={(event) => onCredentialValueChange(event.target.value)}
                placeholder={intl.formatMessage({ id: "bots.feishuAppSecretPlaceholder" })}
                className="min-w-0 flex-1"
                disabled={secretSaving}
              />
              <Button
                variant="outline"
                size="lg"
                onClick={() => onSaveManual(appIdDraft.trim(), credentialValue)}
                disabled={!canSaveManual}
              >
                {secretSaving ? (
                  <LoaderCircle className="size-4 animate-spin" />
                ) : (
                  <KeyRound className="size-4" />
                )}
                {intl.formatMessage({ id: "bots.feishuSaveManual" })}
              </Button>
            </div>
          </div>
        </div>
      </div>
    </DetailPanel>
  );
}

/**
 * 企业微信配置面板：两条并行的接入路径。
 * - 扫码：调用企业微信智能机器人一键创建接口，扫码后自动拿到 botId + secret；
 * - 手动：用户在企业微信管理后台自建机器人后，手动填写 botId 与 secret。
 */
function WeComSetupPanel({
  bot,
  registration,
  registrationLoading,
  credentialValue,
  secretSaving,
  onCredentialValueChange,
  onStartRegistration,
  onSaveManual,
}: {
  bot: BotConfig;
  registration: WecomRegistrationState | null;
  registrationLoading: boolean;
  credentialValue: string;
  secretSaving: boolean;
  onCredentialValueChange: (value: string) => void;
  onStartRegistration: () => void;
  onSaveManual: (wecomBotId: string, secret: string) => void;
}) {
  const { intl } = useModeIntl();
  const [botIdDraft, setBotIdDraft] = useState(bot.wecomBotId ?? "");
  const active = registration?.botId === bot.id ? registration : null;
  const canSaveManual = Boolean(botIdDraft.trim() && credentialValue.trim()) && !secretSaving;

  return (
    <DetailPanel>
      <div className="space-y-4 text-ui-base text-foreground-subtle">
        <div className="flex flex-wrap items-start gap-4">
          {active?.qrDataUrl ? (
            <img
              src={active.qrDataUrl}
              alt={intl.formatMessage({ id: "bots.wecom.registrationQrAlt" })}
              className="size-40 shrink-0 rounded-lg border border-border bg-surface p-2"
            />
          ) : (
            <div className="flex size-40 shrink-0 items-center justify-center rounded-lg border border-dashed border-border bg-surface">
              <QrCode className="size-8 text-foreground-subtle" />
            </div>
          )}
          <div className="min-w-52 flex-1 space-y-2">
            <div className="text-foreground">
              {intl.formatMessage({ id: "bots.wecom.registrationTitle" })}
            </div>
            <div>{intl.formatMessage({ id: "bots.wecom.registrationDescription" })}</div>
            {active ? (
              <div className="text-foreground-subtle">
                {active.status === "pending" || active.status === "scanned"
                  ? intl.formatMessage({ id: `bots.wecom.registration.${active.status}` })
                  : (active.message ??
                    intl.formatMessage({ id: `bots.wecom.registration.${active.status}` }))}
              </div>
            ) : null}
            <Button variant="outline" size="lg" onClick={onStartRegistration} disabled={registrationLoading}>
              {registrationLoading ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : (
                <QrCode className="size-4" />
              )}
              {intl.formatMessage({
                id: active
                  ? "bots.wecom.registrationRefresh"
                  : "bots.wecom.registrationAction",
              })}
            </Button>
          </div>
        </div>

        <div className="border-t border-border pt-3">
          <div className="text-foreground">
            {intl.formatMessage({ id: "bots.wecom.manualTitle" })}
          </div>
          <div className="mt-1">{intl.formatMessage({ id: "bots.wecom.manualDescription" })}</div>
          <div className="mt-2 grid gap-2">
            <Input
              size="lg"
              value={botIdDraft}
              onChange={(event) => setBotIdDraft(event.target.value)}
              placeholder={intl.formatMessage({ id: "bots.wecom.botIdPlaceholder" })}
              disabled={secretSaving}
            />
            <div className="flex w-full min-w-0 items-center gap-2">
              <Input
                size="lg"
                type="password"
                value={credentialValue}
                onChange={(event) => onCredentialValueChange(event.target.value)}
                placeholder={intl.formatMessage({ id: "bots.wecom.secretPlaceholder" })}
                className="min-w-0 flex-1"
                disabled={secretSaving}
              />
              <Button
                variant="outline"
                size="lg"
                onClick={() => onSaveManual(botIdDraft.trim(), credentialValue)}
                disabled={!canSaveManual}
              >
                {secretSaving ? (
                  <LoaderCircle className="size-4 animate-spin" />
                ) : (
                  <KeyRound className="size-4" />
                )}
                {intl.formatMessage({ id: "bots.wecom.saveManual" })}
              </Button>
            </div>
          </div>
        </div>
      </div>
    </DetailPanel>
  );
}

/**
 * 钉钉凭据面板：AppKey（clientId，明文）+ AppSecret（加密凭据）。
 * robotCode 与 clientId 相同，由服务层的发送路径直接复用 clientId。
 */
function DingTalkCredentialPanel({
  bot,
  credentialValue,
  secretSaving,
  onCredentialValueChange,
  onSave,
}: {
  bot: BotConfig;
  credentialValue: string;
  secretSaving: boolean;
  onCredentialValueChange: (value: string) => void;
  onSave: (clientId: string, secret: string) => void;
}) {
  const { intl } = useModeIntl();
  const [clientIdDraft, setClientIdDraft] = useState(bot.dingtalkClientId ?? "");
  const canSave = Boolean(clientIdDraft.trim() && credentialValue.trim()) && !secretSaving;
  return (
    <DetailPanel>
      <div className="space-y-3 text-ui-base text-foreground-subtle">
        <div>{intl.formatMessage({ id: "bots.dingtalk.credentialHint" })}</div>
        <Input
          size="lg"
          value={clientIdDraft}
          onChange={(event) => setClientIdDraft(event.target.value)}
          placeholder={intl.formatMessage({ id: "bots.dingtalk.clientIdPlaceholder" })}
          disabled={secretSaving}
        />
        <div className="flex w-full min-w-0 items-center gap-2">
          <Input
            size="lg"
            type="password"
            value={credentialValue}
            onChange={(event) => onCredentialValueChange(event.target.value)}
            placeholder={intl.formatMessage({ id: "bots.dingtalk.secretPlaceholder" })}
            className="min-w-0 flex-1"
            disabled={secretSaving}
          />
          <Button
            variant="outline"
            size="lg"
            onClick={() => onSave(clientIdDraft.trim(), credentialValue)}
            disabled={!canSave}
          >
            {secretSaving ? (
              <LoaderCircle className="size-4 animate-spin" />
            ) : (
              <KeyRound className="size-4" />
            )}
            {intl.formatMessage({ id: "bots.saveSecret" })}
          </Button>
        </div>
      </div>
    </DetailPanel>
  );
}

/** 绑定码面板：所有 provider 共用（发送 /bind <code> 完成绑定）。 */
export function BindCodePanel({
  bindCode,
  bindExpired,
  bindRemainingMs,
  bindCountdownProgress,
  onCreateBindCode,
  onCopyBindCommand,
}: {
  bindCode: BindCodeState;
  bindExpired: boolean;
  bindRemainingMs: number;
  bindCountdownProgress: number;
  onCreateBindCode: () => void;
  onCopyBindCommand: () => void;
}) {
  const { intl } = useModeIntl();
  const bindCommand = `/bind ${bindCode.code}`;
  return (
    <DetailPanel>
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2 text-ui-base font-medium text-foreground">
            <span className="shrink-0">{intl.formatMessage({ id: "bots.bindCommand" })}</span>
            {/* 绑定码属于哪个 bot：名称可能重复或为空，这里直接亮出 ID。 */}
            <span
              className="min-w-0 truncate font-mono text-ui-sm font-normal text-foreground-subtle"
              title={bindCode.botId}
            >
              {bindCode.botId}
            </span>
          </div>
          <div className="mt-1 text-ui-base leading-5 text-foreground-subtle">
            {intl.formatMessage({ id: "bots.bindCommandGuide" })}
          </div>
        </div>
        <Button variant="ghost" size="sm" onClick={onCreateBindCode}>
          <QrCode className="size-3" />
          {intl.formatMessage({ id: "bots.setup.refreshBindCode" })}
        </Button>
      </div>
      <div className="mt-3 grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md bg-surface px-2 py-1.5">
        <span
          className={cn(
            "min-w-0 break-all font-mono text-ui-base leading-5",
            bindExpired ? "text-foreground-subtle" : "text-foreground",
          )}
        >
          {bindCommand}
        </span>
        <Button
          variant="ghost"
          size="sm"
          onClick={onCopyBindCommand}
          disabled={bindExpired}
          title={intl.formatMessage({ id: "bots.copyBindCommand" })}
        >
          <Copy className="size-4" />
          {intl.formatMessage({ id: "bots.copyBindCommand" })}
        </Button>
      </div>
      <ol className="mt-2 list-decimal space-y-1 ps-4 text-ui-base leading-5 text-foreground-subtle">
        <li>{intl.formatMessage({ id: "bots.bindCommandStep.copy" })}</li>
        <li>{intl.formatMessage({ id: "bots.bindCommandStep.openChat" })}</li>
        <li>{intl.formatMessage({ id: "bots.bindCommandStep.send" })}</li>
      </ol>
      <div className="mt-2 flex items-center gap-2 text-ui-base text-foreground-subtle">
        <Clock3 className="size-3" />
        {bindExpired
          ? intl.formatMessage({ id: "bots.bindCodeExpired" })
          : intl.formatMessage(
              { id: "bots.bindCodeExpires" },
              { time: formatBindCountdown(bindRemainingMs) },
            )}
      </div>
      <div className="mt-2 h-1 overflow-hidden rounded-full bg-surface">
        <div
          key={bindCode.code}
          className={cn(
            "h-full origin-left rounded-full transition-transform duration-300 ease-linear",
            bindExpired
              ? "bg-border"
              : bindCountdownProgress <= 10
                ? "bg-destructive"
                : bindRemainingMs <= 10_000
                  ? "bg-warning"
                  : "bg-primary",
          )}
          style={{ transform: `scaleX(${bindCountdownProgress / 100})` }}
        />
      </div>
    </DetailPanel>
  );
}

export function ProviderSettingsCard({
  bot,
  runtime,
  credentialValue,
  feishuRegistration,
  feishuRegistrationLoading,
  weixinRegistration,
  weixinRegistrationLoading,
  weixinActivated,
  wecomRegistration,
  wecomRegistrationLoading,
  onSaveDingtalkManual,
  secretSaving,
  onCredentialValueChange,
  onRemoveSecret,
  onStartWeixinRegistration,
  onStartFeishuRegistration,
  onStartWecomRegistration,
  onSaveWecomManual,
  onSaveFeishuManual,
}: {
  bot: BotConfig;
  runtime: BotServiceStatus["botRuntime"][number] | undefined;
  credentialValue: string;
  feishuRegistration: FeishuRegistrationState | null;
  feishuRegistrationLoading: boolean;
  weixinRegistration: WeixinRegistrationState | null;
  weixinRegistrationLoading: boolean;
  weixinActivated: boolean;
  wecomRegistration: WecomRegistrationState | null;
  onSaveDingtalkManual: (clientId: string, secret: string) => void;
  wecomRegistrationLoading: boolean;
  onStartWecomRegistration: () => void;
  onSaveWecomManual: (wecomBotId: string, secret: string) => void;
  secretSaving: boolean;
  onCredentialValueChange: (value: string) => void;
  onRemoveSecret: () => void;
  onStartWeixinRegistration: () => void;
  onStartFeishuRegistration: () => void;
  onSaveFeishuManual: (feishuAppId: string, secret: string) => void;
}) {
  const { intl } = useModeIntl();
  if (bot.provider === "webhook") {
    return null;
  }
  const hasSecret = Boolean(bot.credentialRef);
  const isFeishuLike = isFeishuBotProvider(bot.provider);
  const isWeixin = bot.provider === "weixin";
  const isConnected = isWeixin ? hasSecret : Boolean(bot.providerUserId);
  const hasRuntimeError = runtime?.status === "error";
  const descriptionText =
    isFeishuLike && hasRuntimeError
      ? intl.formatMessage({
          id: isConnected
            ? "bots.runtime.boundConnectionInterruptedDescription"
            : "bots.runtime.credentialsSavedConnectionFailedDescription",
        })
      : intl.formatMessage({
          id: `bots.botTokenDescription.${bot.provider}`,
        });
  const hasActiveWeixinRegistration = weixinRegistration?.botId === bot.id;
  // 接入三段状态（spec R1 / R3-4）：判定收在 lib/botOnboardingStages 的纯函数里，卡片只渲染。
  const onboardingStages = deriveBotOnboardingStages({
    bot,
    runtimeStatus: runtime?.status,
    weixinActivated,
  });
  // 凭据未就绪时不显示状态行：那时 control 槽位本身就是第一段的状态与行动入口，
  // 再加一行只是同义重复（spec R3-4「未接管的部分」）。
  const stageRow =
    onboardingStages[0]?.state === "done" ? (
      <BotOnboardingStageRow stages={onboardingStages} />
    ) : null;

  let control: ReactNode = null;
  let detail: ReactNode = null;

  if (isConnected) {
    control = (
      <div className="flex items-center justify-end gap-3">
        {stageRow}
        <Button variant="outline" size="lg" onClick={onRemoveSecret}>
          <Unlink className="size-4" />
          {intl.formatMessage({ id: "bots.removeSecret" })}
        </Button>
      </div>
    );
    if (isWeixin && !weixinActivated) {
      detail = (
        <DetailPanel>
          <div className="text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "bots.weixinActivationHint" })}
          </div>
        </DetailPanel>
      );
    }
  } else if (bot.provider === "dingtalk" && (!hasSecret || !bot.dingtalkClientId)) {
    control = (
      <div className="flex items-center gap-2">
        <span className="text-ui-base text-foreground-subtle">
          {intl.formatMessage({ id: "bots.dingtalk.credentialsMissing" })}
        </span>
      </div>
    );
    detail = (
      <DingTalkCredentialPanel
        bot={bot}
        credentialValue={credentialValue}
        secretSaving={secretSaving}
        onCredentialValueChange={onCredentialValueChange}
        onSave={onSaveDingtalkManual}
      />
    );
  } else if (bot.provider === "wecom" && !hasSecret) {
    control = (
      <div className="flex items-center gap-2">
        <span className="text-ui-base text-foreground-subtle">
          {intl.formatMessage({ id: "bots.wecom.credentialsMissing" })}
        </span>
      </div>
    );
    detail = (
      <WeComSetupPanel
        bot={bot}
        registration={wecomRegistration}
        registrationLoading={wecomRegistrationLoading}
        credentialValue={credentialValue}
        secretSaving={secretSaving}
        onCredentialValueChange={onCredentialValueChange}
        onStartRegistration={onStartWecomRegistration}
        onSaveManual={onSaveWecomManual}
      />
    );
  } else if (isFeishuLike && !hasSecret) {
    // 飞书无凭据：扫码与手填两个入口同时在面板内可用的（spec R3-1）。
    control = (
      <div className="flex items-center gap-2">
        <span className="text-ui-base text-foreground-subtle">
          {intl.formatMessage({ id: "bots.feishuCredentialsMissing" })}
        </span>
      </div>
    );
    detail = (
      <FeishuSetupPanel
        bot={bot}
        registration={feishuRegistration}
        registrationLoading={feishuRegistrationLoading}
        credentialValue={credentialValue}
        secretSaving={secretSaving}
        onCredentialValueChange={onCredentialValueChange}
        onStartRegistration={onStartFeishuRegistration}
        onSaveManual={onSaveFeishuManual}
      />
    );
  } else if (isWeixin && !hasSecret) {
    control = (
      <Button
        variant="outline"
        size="lg"
        onClick={onStartWeixinRegistration}
        disabled={weixinRegistrationLoading}
      >
        {weixinRegistrationLoading ? (
          <LoaderCircle className="size-4 animate-spin" />
        ) : (
          <QrCode className="size-4" />
        )}
        {intl.formatMessage({ id: "bots.scanQrCode" })}
      </Button>
    );
  } else {
    control = (
      <div className="flex w-full flex-wrap items-center justify-end gap-2">
        {stageRow}
        <Button variant="outline" size="lg" onClick={onRemoveSecret}>
          {intl.formatMessage({ id: "bots.removeSecret" })}
        </Button>
      </div>
    );
  }

  if (isFeishuLike && hasRuntimeError) {
    const errorCode = runtime.message?.match(/\b\d{7,}\b/)?.[0];
    detail = (
      <DetailPanel>
        <div className="flex items-start gap-2">
          <CircleAlert className="mt-0.5 size-4 shrink-0 text-destructive" />
          <div className="min-w-0 flex-1">
            <div className="text-ui-base font-medium text-foreground">
              {intl.formatMessage({
                id: isConnected
                  ? bot.provider === "lark"
                    ? "bots.runtime.larkConnectionInterrupted"
                    : "bots.runtime.feishuConnectionInterrupted"
                  : bot.provider === "lark"
                    ? "bots.runtime.cannotConnectLark"
                    : "bots.runtime.cannotConnectFeishu",
              })}
            </div>
            <div className="mt-1 text-ui-base leading-5 text-foreground-subtle">
              {intl.formatMessage({
                id: isConnected
                  ? bot.provider === "lark"
                    ? "bots.runtime.larkConnectionRecoverySuggestion"
                    : "bots.runtime.feishuConnectionRecoverySuggestion"
                  : "bots.runtime.connectionBlocksBinding",
              })}
            </div>
            <dl className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-ui-base leading-5">
              {errorCode ? (
                <>
                  <dt className="text-foreground-subtle">
                    {intl.formatMessage({ id: "bots.runtime.errorCode" })}
                  </dt>
                  <dd className="min-w-0 break-all font-mono text-foreground">{errorCode}</dd>
                </>
              ) : null}
              <dt className="text-foreground-subtle">
                {intl.formatMessage({ id: "bots.runtime.errorDetail" })}
              </dt>
              <dd className="min-w-0 break-words text-foreground">
                {runtime.message ?? intl.formatMessage({ id: "bots.runtime.unknownError" })}
              </dd>
            </dl>
          </div>
        </div>
      </DetailPanel>
    );
  } else if (hasActiveWeixinRegistration) {
    detail = (
      <DetailPanel>
        <div className="flex flex-wrap items-start justify-center gap-4">
          {weixinRegistration.qrDataUrl ? (
            <img
              src={weixinRegistration.qrDataUrl}
              alt={intl.formatMessage({ id: "bots.weixinRegistrationQrAlt" })}
              className="size-40 shrink-0 rounded-lg border border-border bg-surface p-2"
            />
          ) : null}
          <div className="min-w-52 flex-1 space-y-3 text-ui-base text-foreground-subtle">
            <div>{intl.formatMessage({ id: "bots.weixinRegistrationScanHint" })}</div>
            <div>
              {weixinRegistration.status === "pending" ||
              weixinRegistration.status === "scanned" ? (
                <span className="inline-flex items-center gap-1">
                  <LoaderCircle className="size-3 animate-spin" />
                  {intl.formatMessage({
                    id: `bots.weixinRegistration.${weixinRegistration.status}`,
                  })}
                </span>
              ) : (
                (weixinRegistration.message ??
                intl.formatMessage({
                  id: `bots.weixinRegistration.${weixinRegistration.status}`,
                }))
              )}
            </div>
          </div>
        </div>
      </DetailPanel>
    );
  }

  return (
    <SettingsGroupCard>
      <SettingsRow
        label={intl.formatMessage({ id: "bots.botToken" })}
        description={descriptionText}
        control={control}
        detail={
          bot.enabled && runtime?.deliveryError ? (
            <>
              <DetailPanel>
                <div role="alert" className="min-w-0 space-y-2 text-ui-base">
                  <div className="font-medium text-destructive">
                    {intl.formatMessage({ id: "bots.runtime.deliveryFailed" })}
                  </div>
                  <div className="text-foreground-subtle">
                    {intl.formatMessage({ id: "bots.runtime.deliveryFailedDescription" })}
                  </div>
                  <div className="whitespace-pre-wrap break-all text-foreground-subtle">
                    {runtime.deliveryError}
                  </div>
                </div>
              </DetailPanel>
              {detail}
            </>
          ) : (
            detail
          )
        }
      />
    </SettingsGroupCard>
  );
}
