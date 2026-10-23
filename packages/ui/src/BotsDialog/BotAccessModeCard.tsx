import type { BotAccessMode, BotConfig } from "@mode/shared";
import { resolveBotAccessMode, resolveBotGroupChatCapabilities } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/ui-foundation/adapters/select.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 访问模式与黑名单集中管理（docs/specs/bot-access-mode.md R6）：
 * - 访问模式下拉（白名单/自由）统管私聊与群聊准入，值由 resolveBotAccessMode 解析；
 * - 黑名单（blockedUsers / blockedGroups）是两种模式下的纯否决层，这里只展示与解除；
 * - 微信无群聊能力（能力表 false/false），已拉黑群段整段隐藏。
 * 模式切换与解除拉黑都经 props 上抛，服务方法接线在 BotsDialog。
 */
export function BotAccessModeCard({
  bot,
  onSetAccessMode,
  onUnblockUser,
  onUnblockGroup,
}: {
  bot: BotConfig;
  onSetAccessMode: (accessMode: BotAccessMode) => void;
  onUnblockUser: (userId: string) => void;
  onUnblockGroup: (groupId: string) => void;
}) {
  const { intl } = useModeIntl();
  const mode: BotAccessMode = resolveBotAccessMode(bot);
  const blockedUsers = bot.blockedUsers ?? [];
  const blockedGroups = bot.blockedGroups ?? [];
  const capabilities = resolveBotGroupChatCapabilities(bot.provider);
  const supportsGroupChat = capabilities.mention || capabilities.always;

  return (
    <div>
      <SettingsRow
        label={intl.formatMessage({ id: "bots.accessMode.title" })}
        description={intl.formatMessage({ id: "bots.accessMode.description" })}
        control={
          <Select value={mode} onValueChange={(value) => onSetAccessMode(value as BotAccessMode)}>
            {/* 与同面板其它下拉统一宽度：w-48（192px），避免同一列上下参差 */}
            <SelectTrigger size="lg" className="w-48 justify-between">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="whitelist">
                {intl.formatMessage({ id: "bots.accessMode.mode.whitelist" })}
              </SelectItem>
              <SelectItem value="free">
                {intl.formatMessage({ id: "bots.accessMode.mode.free" })}
              </SelectItem>
            </SelectContent>
          </Select>
        }
      />

      <div className="mt-1 space-y-3 px-4 pb-3">
        <div className="space-y-1">
          <p className="text-ui-caption font-medium text-foreground-subtle">
            {intl.formatMessage({ id: "bots.accessMode.blockedUsers" })}
          </p>
          {blockedUsers.length > 0 ? (
            blockedUsers.map((userId) => (
              <div
                key={userId}
                className="flex items-center justify-between gap-3 rounded-lg bg-surface px-3 py-2 text-ui-base"
              >
                <span className="min-w-0 truncate font-mono text-foreground" title={userId}>
                  {userId}
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="shrink-0 text-foreground-subtle hover:text-destructive"
                  onClick={() => onUnblockUser(userId)}
                >
                  {intl.formatMessage({ id: "bots.accessMode.unblockUser" })}
                </Button>
              </div>
            ))
          ) : (
            <p className="text-ui-caption text-foreground-subtle">
              {intl.formatMessage({ id: "bots.accessMode.empty" })}
            </p>
          )}
        </div>

        {supportsGroupChat ? (
          <div className="space-y-1">
            <p className="text-ui-caption font-medium text-foreground-subtle">
              {intl.formatMessage({ id: "bots.accessMode.blockedGroups" })}
            </p>
            {blockedGroups.length > 0 ? (
              blockedGroups.map((group) => (
                <div
                  key={group.groupId}
                  className="flex items-center justify-between gap-3 rounded-lg bg-surface px-3 py-2 text-ui-base"
                >
                  <div className="min-w-0">
                    <p className="truncate text-foreground">{group.groupName || group.groupId}</p>
                    <p className="text-ui-caption text-foreground-subtle">{group.groupId}</p>
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="shrink-0 text-foreground-subtle hover:text-destructive"
                    onClick={() => onUnblockGroup(group.groupId)}
                  >
                    {intl.formatMessage({ id: "bots.accessMode.unblockGroup" })}
                  </Button>
                </div>
              ))
            ) : (
              <p className="text-ui-caption text-foreground-subtle">
                {intl.formatMessage({ id: "bots.accessMode.empty" })}
              </p>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}
