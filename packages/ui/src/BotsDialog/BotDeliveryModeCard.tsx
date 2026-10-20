import type { BotConfig, BotDeliveryMode } from "@mode/shared";
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
 * 渠道输出模式：
 * - auto：AI 文本自动推送到当前会话；
 * - tool_only：普通 AI 输出不外发，必须由显式工具发送。
 * 命令回执、权限/提问等控制消息不受影响。
 */
export function BotDeliveryModeCard({
  bot,
  onPatchBot,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
}) {
  const { intl } = useModeIntl();
  const mode: BotDeliveryMode = bot.deliveryMode ?? "auto";
  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.deliveryMode.title" })}
      layout="stacked"
      description={intl.formatMessage({ id: "bots.deliveryMode.description" })}
      control={
        <Select
          value={mode}
          onValueChange={(value) => onPatchBot({ deliveryMode: value as BotDeliveryMode })}
        >
          <SelectTrigger size="lg" className="w-48 justify-between">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">
              {intl.formatMessage({ id: "bots.deliveryMode.auto" })}
            </SelectItem>
            <SelectItem value="tool_only">
              {intl.formatMessage({ id: "bots.deliveryMode.toolOnly" })}
            </SelectItem>
          </SelectContent>
        </Select>
      }
    />
  );
}
