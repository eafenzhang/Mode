import { X } from "lucide-react";
import type { BotConfig, BotPrivateChatMode } from "@zcode/shared";
import { Button } from "@/components/ui/button.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { BindCodePanel, DetailPanel } from "@/BotsDialog/ProviderSettingsCard.js";
import type { BindCodeState } from "@/BotsDialog/shared.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 私聊方式（对齐 MyAgents 的私聊范围）：
 * - 绑定用户：只有绑定过的用户能驱动机器人；这里给出绑定码与已绑定用户列表。
 * - 全部用户：不需要任何绑定操作，任何私聊用户都能驱动（主动消息投递到最近说话的那位）。
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
}) {
  const { intl } = useZCodeIntl();
  const mode: BotPrivateChatMode = bot.privateChatMode ?? "bound_users";
  const boundUsers = bot.allowedUsers ?? [];
  const primaryUserId = bot.providerUserId?.trim() ?? "";
  const showBindCode = bindCode?.botId === bot.id && !bindExpired;
  const isBoundUsersMode = mode === "bound_users";

  const removeUser = (userId: string) => {
    if (userId === primaryUserId) {
      onUnbind();
      return;
    }
    onPatchBot({ allowedUsers: boundUsers.filter((item) => item !== userId) });
  };

  return (
    <>
      <SettingsRow
        label={intl.formatMessage({ id: "bots.privateChat.title" })}
        description={intl.formatMessage({
          id: isBoundUsersMode
            ? "bots.privateChat.description.boundUsers"
            : "bots.privateChat.description.allUsers",
        })}
        control={
          <Select
            value={mode}
            onValueChange={(value) =>
              onPatchBot({ privateChatMode: value as BotPrivateChatMode })
            }
          >
            {/* 与同面板其它下拉统一宽度：w-48（192px），避免同一列上下参差 */}
            <SelectTrigger size="lg" className="w-48 justify-between">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="bound_users">
                {intl.formatMessage({ id: "bots.privateChat.mode.boundUsers" })}
              </SelectItem>
              <SelectItem value="all_users">
                {intl.formatMessage({ id: "bots.privateChat.mode.allUsers" })}
              </SelectItem>
            </SelectContent>
          </Select>
        }
      />

      {isBoundUsersMode ? (
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
      ) : null}

      {isBoundUsersMode && showBindCode ? (
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
