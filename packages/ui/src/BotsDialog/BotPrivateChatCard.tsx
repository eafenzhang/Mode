import { X } from "lucide-react";
import { resolveBotAccessMode, type BotAccessMode, type BotConfig } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { BindCodePanel, DetailPanel } from "@/BotsDialog/ProviderSettingsCard.js";
import type { BindCodeState } from "@/BotsDialog/shared.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 私聊准入按统一访问模式分支（docs/specs/bot-access-mode.md R6）：
 * - whitelist：仅绑定用户 / allowedUsers 可驱动；这里给出绑定码与已绑定用户列表。
 * - free：任何私聊用户都可驱动并自动登记；这里展示 userRegistry（移除、拉黑）。
 * 模式切换的下拉已移入 BotAccessModeCard；本卡不再持有「私聊方式」开关。
 */
export function BotPrivateChatCard({
  bot,
  bindCode,
  bindExpired,
  bindRemainingMs,
  bindCountdownProgress,
  onCreateBindCode,
  onCopyBindCommand,
  onUnbind,
  onPatchBot,
  onRemoveRegistryUser,
  onBlockUser,
}: {
  bot: BotConfig;
  bindCode: BindCodeState | null;
  bindExpired: boolean;
  bindRemainingMs: number;
  bindCountdownProgress: number;
  onCreateBindCode: () => void;
  onCopyBindCommand: () => void;
  onUnbind: () => void;
  onPatchBot: (patch: Partial<BotConfig>) => void;
  onRemoveRegistryUser: (userId: string) => void;
  onBlockUser: (userId: string) => void;
}) {
  const { intl } = useModeIntl();
  const mode: BotAccessMode = resolveBotAccessMode(bot);
  const isWhitelist = mode === "whitelist";
  const boundUsers = bot.allowedUsers ?? [];
  const primaryUserId = bot.providerUserId?.trim() ?? "";
  const showBindCode = bindCode?.botId === bot.id && !bindExpired;
  const registeredUsers = bot.userRegistry ?? [];

  const removeUser = (userId: string) => {
    if (userId === primaryUserId) {
      onUnbind();
      return;
    }
    onPatchBot({ allowedUsers: boundUsers.filter((item) => item !== userId) });
  };

  return (
    <>
      {isWhitelist ? (
        <SettingsRow
          label={intl.formatMessage({ id: "bots.privateChat.boundUsersLabel" })}
          description={intl.formatMessage({ id: "bots.privateChat.boundUsersHint" })}
          control={
            <div className="flex w-full min-w-0 flex-col items-end gap-2">
              {primaryUserId || boundUsers.length > 0 ? (
                <div className="flex w-full min-w-0 flex-col gap-1" data-bound-users-list="true">
                  {[primaryUserId, ...boundUsers]
                    .filter((userId, index, list) => userId && list.indexOf(userId) === index)
                    .map((userId) => (
                      <span
                        key={userId}
                        className="flex min-w-0 items-center justify-end gap-2 text-ui-base"
                      >
                        <span className="min-w-0 truncate font-mono" title={userId}>
                          {userId}
                        </span>
                        {userId === primaryUserId ? (
                          <span className="shrink-0 text-foreground-subtle">
                            {intl.formatMessage({ id: "bots.privateChat.primaryBadge" })}
                          </span>
                        ) : null}
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          className="shrink-0"
                          aria-label={intl.formatMessage(
                            { id: "bots.privateChat.removeUser" },
                            { user: userId },
                          )}
                          onClick={() => removeUser(userId)}
                        >
                          <X className="size-3.5" />
                        </Button>
                      </span>
                    ))}
                </div>
              ) : (
                <span className="text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "bots.privateChat.noBoundUsers" })}
                </span>
              )}
              {!showBindCode ? (
                <Button variant="outline" size="lg" onClick={onCreateBindCode}>
                  {intl.formatMessage({ id: "bots.bind" })}
                </Button>
              ) : null}
            </div>
          }
        />
      ) : (
        <SettingsRow
          label={intl.formatMessage({ id: "bots.registry.title" })}
          control={
            <div className="flex w-full min-w-0 flex-col items-end gap-2">
              {registeredUsers.length > 0 ? (
                <div className="flex w-full min-w-0 flex-col gap-1">
                  {registeredUsers.map((entry) => {
                    const isBlocked = bot.blockedUsers?.includes(entry.providerUserId) ?? false;
                    return (
                      <span
                        key={entry.providerUserId}
                        className="flex min-w-0 items-center justify-end gap-2 text-ui-base"
                      >
                        <span className="min-w-0 truncate font-mono" title={entry.providerUserId}>
                          {entry.displayName
                            ? `${entry.displayName} · ${entry.providerUserId}`
                            : entry.providerUserId}
                        </span>
                        {isBlocked ? (
                          <span className="shrink-0 text-foreground-subtle">
                            {intl.formatMessage({ id: "bots.registry.blockedBadge" })}
                          </span>
                        ) : null}
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="shrink-0 text-foreground-subtle hover:text-destructive"
                          onClick={() => onRemoveRegistryUser(entry.providerUserId)}
                        >
                          {intl.formatMessage({ id: "bots.registry.remove" })}
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="shrink-0 text-foreground-subtle hover:text-destructive"
                          onClick={() => onBlockUser(entry.providerUserId)}
                        >
                          {intl.formatMessage({ id: "bots.registry.block" })}
                        </Button>
                      </span>
                    );
                  })}
                </div>
              ) : (
                <span className="text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "bots.registry.empty" })}
                </span>
              )}
            </div>
          }
        />
      )}

      {isWhitelist && showBindCode ? (
        <DetailPanel>
          <BindCodePanel
            bindCode={bindCode}
            bindExpired={bindExpired}
            bindRemainingMs={bindRemainingMs}
            bindCountdownProgress={bindCountdownProgress}
            onCreateBindCode={onCreateBindCode}
            onCopyBindCommand={onCopyBindCommand}
          />
        </DetailPanel>
      ) : null}
    </>
  );
}
