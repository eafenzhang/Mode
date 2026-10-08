import { LoaderCircle } from "lucide-react";
import type { BotConfig, BotWorkspaceRef } from "@mode/shared";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/** 「未绑定」选项的哨兵值：选中它即解除工作区绑定，让 bot 按 /项目 自由跟随。 */
export const WORKSPACE_MANAGEMENT_UNBOUND_VALUE = "__unbound__";

/**
 * 工作区管理：用下拉选择该机器人绑定的工作区（一个机器人只能绑一个）。
 * 绑定即授权——访问范围、消息流向、上下文归属都由这一处决定，不再有第二个互相打架的授权入口；
 * 选「未绑定」等价于解绑，bot 恢复按 /项目 自由切换。
 */
export function WorkspaceManagementCard({
  bot,
  boundWorkspace,
  availableWorkspaces,
  loading,
  onSelectWorkspace,
  onUnbind,
}: {
  bot: BotConfig;
  /** 该 bot 当前绑定的工作区（一个机器人至多一个）；未绑定为 null。 */
  boundWorkspace: BotWorkspaceRef | null;
  /** 可选工作区列表（当前已知的全部工作区）。 */
  availableWorkspaces: BotWorkspaceRef[];
  loading: boolean;
  onSelectWorkspace: (workspace: BotWorkspaceRef) => Promise<void>;
  onUnbind: () => Promise<void>;
}) {
  const { intl } = useModeIntl();
  // 已绑定但不在当前列表里的工作区（远端断开、历史路径）仍要作为选项出现，
  // 否则下拉会显示成"未绑定"，而 bot 实际还钉着那个工作区。
  const options = boundWorkspace
    ? [boundWorkspace, ...availableWorkspaces.filter((item) => item.id !== boundWorkspace.id)]
    : availableWorkspaces;
  const value = boundWorkspace?.id ?? WORKSPACE_MANAGEMENT_UNBOUND_VALUE;

  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.workspaceManagement.title" })}
      description={intl.formatMessage(
        {
          id: boundWorkspace
            ? "bots.workspaceManagement.description.bound"
            : "bots.workspaceManagement.description.unbound",
        },
        { botId: bot.id, workspace: boundWorkspace?.label ?? "" },
      )}
      control={
        <div className="flex w-full min-w-0 items-center justify-end gap-2">
          {loading ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-foreground-subtle" />
          ) : null}
          <Select
            value={value}
            disabled={loading}
            onValueChange={(next) => {
              if (next === WORKSPACE_MANAGEMENT_UNBOUND_VALUE) {
                void onUnbind();
                return;
              }
              const target = options.find((item) => item.id === next);
              if (target) {
                void onSelectWorkspace(target);
              }
            }}
          >
            {/* 与同面板其它下拉统一宽度：w-48（192px） */}
            <SelectTrigger size="lg" className="w-48 justify-between">
              <SelectValue
                placeholder={intl.formatMessage({
                  id: "bots.workspaceManagement.selectPlaceholder",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={WORKSPACE_MANAGEMENT_UNBOUND_VALUE}>
                {intl.formatMessage({ id: "bots.workspaceManagement.option.unbound" })}
              </SelectItem>
              {options.map((workspace) => (
                <SelectItem key={workspace.id} value={workspace.id}>
                  {workspace.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      }
    />
  );
}
