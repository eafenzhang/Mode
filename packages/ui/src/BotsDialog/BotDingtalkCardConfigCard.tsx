import { useState } from "react";
import type { BotConfig } from "@mode/shared";
import { Input } from "@/components/ui/input.js";
import { Switch } from "@/components/ui/switch.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 钉钉 AI 卡片配置（对齐 MyAgents 的 useAiCard + cardTemplateId）：
 * 开启并填写卡片模板 ID 后，回复走卡片流式更新；未配置时自动回退 Markdown 摘要。
 * 模板在钉钉开放平台的「卡片平台」创建，卡片参数需包含 content 字段。
 */
export function BotDingtalkCardConfigCard({
  bot,
  onPatchBot,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
}) {
  const { intl } = useModeIntl();
  const enabled = bot.dingtalkUseAiCard === true;
  const [templateDraft, setTemplateDraft] = useState<string | null>(null);
  const templateValue = templateDraft ?? bot.dingtalkCardTemplateId ?? "";

  const commitTemplate = () => {
    setTemplateDraft(null);
    const next = templateValue.trim();
    if (next === (bot.dingtalkCardTemplateId ?? "")) {
      return;
    }
    onPatchBot({ dingtalkCardTemplateId: next || undefined });
  };

  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.dingtalkCard.title" })}
      description={intl.formatMessage({ id: "bots.dingtalkCard.description" })}
      control={
        <div className="flex w-full min-w-0 items-center justify-end gap-2">
          <Input
            size="lg"
            value={templateValue}
            onChange={(event) => setTemplateDraft(event.target.value)}
            onBlur={commitTemplate}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.currentTarget.blur();
              }
            }}
            placeholder={intl.formatMessage({ id: "bots.dingtalkCard.templatePlaceholder" })}
            className="min-w-0 flex-1"
            disabled={!enabled}
          />
          <Switch
            checked={enabled}
            aria-label={intl.formatMessage({ id: "bots.dingtalkCard.title" })}
            onCheckedChange={(checked) => onPatchBot({ dingtalkUseAiCard: checked })}
          />
        </div>
      }
    />
  );
}
