import type { BotConfig, BotGroupActivation } from "@mode/shared";
import { resolveBotGroupChatCapabilities } from "@mode/shared";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 群聊激活模式：默认 disabled（与旧行为一致，群消息静默忽略）。选项由平台能力决定：
 * - 能识别"是否被 @" → 提供 @提及；
 * - 能收到未被 @ 的群消息 → 提供 全部消息（企微只在被 @ 时下发回调，因此不提供）；
 * 「全部消息」模式下模型可用 <NO_REPLY> 主动保持沉默，群内默认禁用 Bash/Edit/Write。
 */
export function BotGroupChatCard({
  bot,
  onPatchBot,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
}) {
  const { intl } = useModeIntl();
  const capabilities = resolveBotGroupChatCapabilities(bot.provider);
  // 平台只有 mention 语义时（企微），存量配置里的 always 不会再收到未 @ 的消息：
  // 显示层收敛到 mention，避免用户以为自己在用"全部消息"。
  const configured: BotGroupActivation = bot.groupChat?.activation ?? "disabled";
  const activation: BotGroupActivation =
    !capabilities.always && configured === "always" ? "mention" : configured;
  const boundUser = Boolean(bot.providerUserId?.trim());
  // 群聊"谁能驱动"跟随「私聊方式」：绑定用户 → 仅绑定用户（未绑定时群消息静默忽略）；
  // 全部用户 → 群里任何成员都能驱动，无需绑定。文案必须如实覆盖两种模式。
  const privateAllUsers = (bot.privateChatMode ?? "bound_users") === "all_users";

  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.groupChat.title" })}
      description={intl.formatMessage(
        {
          id:
            !boundUser && !privateAllUsers
              ? "bots.groupChat.descriptionUnbound"
              : !capabilities.always
                ? // 平台只有 mention 语义（企微）：先说清平台限制
                  "bots.groupChat.descriptionWecom"
                : privateAllUsers
                  ? activation === "always"
                    ? "bots.groupChat.descriptionAllUsersAlways"
                    : "bots.groupChat.descriptionAllUsers"
                  : activation === "always"
                    ? "bots.groupChat.descriptionAlways"
                    : capabilities.mention
                      ? "bots.groupChat.description"
                      : "bots.groupChat.descriptionNoMention",
        },
        {},
      )}
      control={
        <Select
          value={activation}
          onValueChange={(value) =>
            onPatchBot({ groupChat: { activation: value as BotGroupActivation } })
          }
        >
          <SelectTrigger size="lg" className="w-48 justify-between">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="disabled">
              {intl.formatMessage({ id: "bots.groupChat.mode.disabled" })}
            </SelectItem>
            {capabilities.mention ? (
              <SelectItem value="mention">
                {intl.formatMessage({ id: "bots.groupChat.mode.mention" })}
              </SelectItem>
            ) : null}
            {capabilities.always ? (
              <SelectItem value="always">
                {intl.formatMessage({ id: "bots.groupChat.mode.always" })}
              </SelectItem>
            ) : null}
          </SelectContent>
        </Select>
      }
    />
  );
}
