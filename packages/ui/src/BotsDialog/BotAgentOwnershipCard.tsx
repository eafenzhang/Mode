import type { BotConfig, BotWorkspaceRef } from "@mode/shared";
import { resolveBotAgentId } from "@mode/shared";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * Agent 归属（只读投影）：A+ 迁移默认一 Bot 一 Agent。
 * v4 落地前不引入第二份状态，只展示派生 Agent id 与当前绑定工作区；
 * Agent/Channel 拆分与可编辑归属由 v4 迁移替换。
 */
export function BotAgentOwnershipCard({
  bot,
  boundWorkspace,
}: {
  bot: BotConfig;
  boundWorkspace: BotWorkspaceRef | null;
}) {
  const { intl } = useModeIntl();
  const workspace = boundWorkspace?.label?.trim() || boundWorkspace?.workspacePath?.trim();
  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.agentOwnership.title" })}
      description={intl.formatMessage({ id: "bots.agentOwnership.description" })}
      control={
        <div className="min-w-0 space-y-0.5 text-end">
          <p className="truncate text-sm text-[var(--ink)]">{resolveBotAgentId(bot.id)}</p>
          <p className="truncate text-xs text-[var(--ink-muted)]">
            {workspace ?? intl.formatMessage({ id: "bots.agentOwnership.unbound" })}
          </p>
        </div>
      }
    />
  );
}
