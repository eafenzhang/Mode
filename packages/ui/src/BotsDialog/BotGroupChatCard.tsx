import { useState } from "react";
import type { BotConfig, BotGroupActivation } from "@mode/shared";
import { resolveBotAccessMode, resolveBotGroupChatCapabilities } from "@mode/shared";
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
 *
 * 访问模式分支（docs/specs/bot-access-mode.md R6）：
 * - whitelist：保留 pending→批准 / approved→移除 的审批流（现行为）；
 * - free：不出现审批按钮，改为展示自动登记群全量列表（含 pending 遗留），可拉黑、可移除。
 * 激活模式下拉两模式都保留。
 */
export function BotGroupChatCard({
  bot,
  onPatchBot,
  onApproveGroup,
  onRejectGroup,
  onRemoveGroup,
  onBlockGroup,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
  onApproveGroup?: (groupId: string) => Promise<void>;
  onRejectGroup?: (groupId: string) => Promise<void>;
  onRemoveGroup?: (groupId: string) => Promise<void>;
  onBlockGroup?: (groupId: string, groupName: string) => void;
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
  // free 模式下私聊免绑定，描述文案不再走「请先 /bind」分支。
  const accessMode = resolveBotAccessMode(bot);
  const freeMode = accessMode === "free";
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
    <div>
      <SettingsRow
        label={intl.formatMessage({ id: "bots.groupChat.title" })}
        description={intl.formatMessage(
          {
            id:
              !boundUser && !freeMode
                ? "bots.groupChat.descriptionUnbound"
                : !capabilities.always
                  ? "bots.groupChat.descriptionWecom"
                  : freeMode
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

      <div className="mt-1 space-y-3 px-4 pb-3">
        {actionError ? <p className="text-ui-caption text-destructive">{actionError}</p> : null}

        {freeMode ? (
          <div className="space-y-2">
            <p className="text-ui-caption font-medium text-foreground-subtle">
              {intl.formatMessage({ id: "bots.groupChat.registered" })}
            </p>
            {permissions.map((permission) => (
              <div
                key={permission.groupId}
                className="flex items-center justify-between gap-3 rounded-lg bg-surface px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate text-ui-base text-foreground">
                    {permission.groupName || permission.groupId}
                  </p>
                  <p className="text-ui-caption text-foreground-subtle">{permission.groupId}</p>
                </div>
                <div className="flex shrink-0 gap-1.5">
                  <button
                    type="button"
                    disabled={!onBlockGroup}
                    onClick={() => onBlockGroup?.(permission.groupId, permission.groupName)}
                    className="rounded-md bg-surface px-2.5 py-1 text-ui-caption text-foreground hover:text-destructive disabled:opacity-50"
                  >
                    {intl.formatMessage({ id: "bots.groupChat.block" })}
                  </button>
                  <button
                    type="button"
                    disabled={busyGroupId === permission.groupId || !onRemoveGroup}
                    onClick={() =>
                      void runGroupAction(permission.groupId, () =>
                        onRemoveGroup!(permission.groupId),
                      )
                    }
                    className="rounded-md px-2.5 py-1 text-ui-caption text-foreground hover:text-destructive disabled:opacity-50"
                  >
                    {intl.formatMessage({ id: "bots.groupChat.permissions.remove" })}
                  </button>
                </div>
              </div>
            ))}
            {permissions.length === 0 ? (
              <p className="text-ui-caption text-foreground-subtle">
                {intl.formatMessage({ id: "bots.groupChat.registeredEmpty" })}
              </p>
            ) : null}
          </div>
        ) : (
          <>
            {pending.length > 0 ? (
              <div className="space-y-2">
                <p className="text-ui-caption font-medium text-warning">
                  {intl.formatMessage({ id: "bots.groupChat.permissions.pending" })}
                </p>
                {pending.map((permission) => (
                  <div
                    key={permission.groupId}
                    className="flex items-center justify-between gap-3 rounded-lg border border-warning/20 bg-warning/5 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-ui-base text-foreground">
                        {permission.groupName || permission.groupId}
                      </p>
                      <p className="text-ui-caption text-foreground-subtle">{permission.groupId}</p>
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
                        className="rounded-md bg-primary px-2.5 py-1 text-ui-caption text-primary-foreground disabled:opacity-50"
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
                        className="rounded-md bg-surface px-2.5 py-1 text-ui-caption text-foreground-subtle disabled:opacity-50"
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
                <p className="text-ui-caption font-medium text-foreground-subtle">
                  {intl.formatMessage({ id: "bots.groupChat.permissions.approved" })}
                </p>
                {approved.map((permission) => (
                  <div
                    key={permission.groupId}
                    className="flex items-center justify-between gap-3 rounded-lg bg-surface px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-ui-base text-foreground">
                        {permission.groupName || permission.groupId}
                      </p>
                      <p className="text-ui-caption text-foreground-subtle">{permission.groupId}</p>
                    </div>
                    <button
                      type="button"
                      disabled={busyGroupId === permission.groupId || !onRemoveGroup}
                      onClick={() =>
                        void runGroupAction(permission.groupId, () =>
                          onRemoveGroup!(permission.groupId),
                        )
                      }
                      className="shrink-0 rounded-md px-2.5 py-1 text-ui-caption text-foreground hover:text-destructive disabled:opacity-50"
                    >
                      {intl.formatMessage({ id: "bots.groupChat.permissions.remove" })}
                    </button>
                  </div>
                ))}
              </div>
            ) : null}

            {permissions.length === 0 ? (
              <p className="text-ui-caption text-foreground-subtle">
                {intl.formatMessage({ id: "bots.groupChat.permissions.empty" })}
              </p>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}
