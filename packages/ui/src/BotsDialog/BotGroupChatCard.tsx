import { useState } from "react";
import type { BotConfig, BotGroupActivation } from "@mode/shared";
import { resolveBotGroupChatCapabilities } from "@mode/shared";
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
 * 群聊激活模式：默认 disabled（与旧行为一致，群消息静默忽略）。选项由平台能力决定：
 * - 能识别"是否被 @" → 提供 @提及；
 * - 能收到未被 @ 的群消息 → 提供 全部消息。
 * 微信按 A+ 只保留私聊（能力为 false/false），整个卡片不渲染。
 * 群授权记录由服务端持有；这里只做展示和命令入口。
 */
export function BotGroupChatCard({
  bot,
  onPatchBot,
  onApproveGroup,
  onRejectGroup,
  onRemoveGroup,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
  onApproveGroup?: (groupId: string) => Promise<void>;
  onRejectGroup?: (groupId: string) => Promise<void>;
  onRemoveGroup?: (groupId: string) => Promise<void>;
}) {
  const { intl } = useModeIntl();
  const [busyGroupId, setBusyGroupId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const capabilities = resolveBotGroupChatCapabilities(bot.provider);
  if (!capabilities.mention && !capabilities.always) {
    return null;
  }
  // 平台只有 mention 语义时（企微/钉钉），存量配置里的 always 不会再收到未 @ 的消息：
  // 显示层收敛到 mention，避免用户以为自己在用"全部消息"。
  const configured: BotGroupActivation = bot.groupChat?.activation ?? "disabled";
  const activation: BotGroupActivation =
    !capabilities.always && configured === "always" ? "mention" : configured;
  const boundUser = Boolean(bot.providerUserId?.trim());
  const privateAllUsers = (bot.privateChatMode ?? "bound_users") === "all_users";
  const permissions = bot.groupChat?.permissions ?? [];
  const pending = permissions.filter((permission) => permission.status === "pending");
  const approved = permissions.filter((permission) => permission.status === "approved");

  const runGroupAction = async (groupId: string, action: () => Promise<void>) => {
    setBusyGroupId(groupId);
    setActionError(null);
    try {
      await action();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyGroupId(null);
    }
  };

  return (
    <div className="space-y-3">
      <SettingsRow
        label={intl.formatMessage({ id: "bots.groupChat.title" })}
        description={intl.formatMessage(
          {
            id:
              !boundUser && !privateAllUsers
                ? "bots.groupChat.descriptionUnbound"
                : !capabilities.always
                  ? "bots.groupChat.descriptionWecom"
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
              onPatchBot({
                groupChat: { ...bot.groupChat, activation: value as BotGroupActivation },
              })
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

      {actionError ? <p className="text-xs text-[var(--error)]">{actionError}</p> : null}

      {pending.length > 0 ? (
        <div className="space-y-2">
          <p className="text-xs font-medium text-[var(--warning)]">
            {intl.formatMessage({ id: "bots.groupChat.permissions.pending" })}
          </p>
          {pending.map((permission) => (
            <div
              key={permission.groupId}
              className="flex items-center justify-between gap-3 rounded-lg border border-[var(--warning)]/20 bg-[var(--warning)]/5 px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate text-sm text-[var(--ink)]">
                  {permission.groupName || permission.groupId}
                </p>
                <p className="text-xs text-[var(--ink-muted)]">{permission.groupId}</p>
              </div>
              <div className="flex shrink-0 gap-1.5">
                <button
                  type="button"
                  disabled={busyGroupId === permission.groupId || !onApproveGroup}
                  onClick={() =>
                    void runGroupAction(permission.groupId, () =>
                      onApproveGroup!(permission.groupId),
                    )
                  }
                  className="rounded-md bg-[var(--accent)] px-2.5 py-1 text-xs text-[var(--on-accent)] disabled:opacity-50"
                >
                  {intl.formatMessage({ id: "bots.groupChat.permissions.approve" })}
                </button>
                <button
                  type="button"
                  disabled={busyGroupId === permission.groupId || !onRejectGroup}
                  onClick={() =>
                    void runGroupAction(permission.groupId, () =>
                      onRejectGroup!(permission.groupId),
                    )
                  }
                  className="rounded-md bg-[var(--paper-inset)] px-2.5 py-1 text-xs text-[var(--ink-muted)] disabled:opacity-50"
                >
                  {intl.formatMessage({ id: "bots.groupChat.permissions.reject" })}
                </button>
              </div>
            </div>
          ))}
        </div>
      ) : null}

      {approved.length > 0 ? (
        <div className="space-y-2">
          <p className="text-xs font-medium text-[var(--ink-muted)]">
            {intl.formatMessage({ id: "bots.groupChat.permissions.approved" })}
          </p>
          {approved.map((permission) => (
            <div
              key={permission.groupId}
              className="flex items-center justify-between gap-3 rounded-lg bg-[var(--paper-inset)] px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate text-sm text-[var(--ink)]">
                  {permission.groupName || permission.groupId}
                </p>
                <p className="text-xs text-[var(--ink-muted)]">{permission.groupId}</p>
              </div>
              <button
                type="button"
                disabled={busyGroupId === permission.groupId || !onRemoveGroup}
                onClick={() =>
                  void runGroupAction(permission.groupId, () => onRemoveGroup!(permission.groupId))
                }
                className="shrink-0 rounded-md px-2.5 py-1 text-xs text-[var(--ink-muted)] hover:text-[var(--error)] disabled:opacity-50"
              >
                {intl.formatMessage({ id: "bots.groupChat.permissions.remove" })}
              </button>
            </div>
          ))}
        </div>
      ) : null}

      {permissions.length === 0 ? (
        <p className="text-xs text-[var(--ink-muted)]">
          {intl.formatMessage({ id: "bots.groupChat.permissions.empty" })}
        </p>
      ) : null}
    </div>
  );
}
