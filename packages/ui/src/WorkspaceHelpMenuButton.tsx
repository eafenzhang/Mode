import {
  DesktopCommandIds,
  TID_WORKSPACE_HELP_MENU_TRIGGER,
  type DesktopCommandId,
} from "@mode/shared";
import { CircleHelpIcon, InfoIcon, RefreshCwIcon, SquareLibraryIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu.js";
import { cn } from "@/components/lib/utils.js";
import { ControlHintTooltip } from "@/ControlHintTooltip.js";
import { useDesktopUpdateMenu } from "@/hooks/useDesktopUpdateMenu.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";

/**
 * 右上角「帮助」入口：菜单里只保留三项——资源管理器、检查更新、关于 Mode。
 * 社区 / 问题反馈 / 功能建议 / 导出日志 / 开发者工具 / 清除数据等入口已下线（程序更新由静默更新承担，
 * 手动检查更新只作为明确入口保留）。菜单标题与条目文案复用既有 i18n 键，不再新增。
 * Web 端没有资源管理器窗口，不渲染。
 */
export function WorkspaceHelpMenuButton({
  className,
  isDesktop = false,
}: {
  className?: string;
  /**
   * 是否桌面端。由挂载处注入而不是在组件内嗅探：Web 的 IPlatformService 桩同样实现了
   * executeDesktopCommand（no-op），拿它判定会让 Web 端出现一个点了没反应的「帮助」。
   */
  isDesktop?: boolean;
}) {
  const { intl } = useZCodeIntl();
  const platform = usePlatform();
  const updateMenu = useDesktopUpdateMenu(isDesktop);
  if (!isDesktop) {
    return null;
  }
  const menuLabel = intl.formatMessage({ id: "workspaceHeader.help.menu" });
  const execute = (command: DesktopCommandId) => {
    void platform.executeDesktopCommand(command);
  };

  return (
    <DropdownMenu>
      <ControlHintTooltip title={menuLabel} side="bottom">
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-md"
            // Settings 页会把入口绝对定位在 Electron 顶部拖拽区上方。
            // 只依赖外层容器 no-drag 时，真实 trigger 仍可能被标题栏 drag 区吞掉点击。
            className={cn(
              "text-foreground hover:bg-hover hover:text-foreground [app-region:no-drag]",
              className,
            )}
            aria-label={menuLabel}
            data-testid={TID_WORKSPACE_HELP_MENU_TRIGGER}
          >
            <CircleHelpIcon className="size-4" />
          </Button>
        </DropdownMenuTrigger>
      </ControlHintTooltip>
      <DropdownMenuContent align="end" className="[app-region:no-drag]">
        <DropdownMenuItem
          data-testid="workspace-help-menu-resource-manager"
          onSelect={() => execute(DesktopCommandIds.OpenResourceManager)}
        >
          <SquareLibraryIcon className="size-4" aria-hidden="true" />
          {intl.formatMessage({ id: "titleBar.menu.help.resourceManager" })}
        </DropdownMenuItem>
        <DropdownMenuItem
          data-testid="workspace-help-menu-check-for-updates"
          disabled={updateMenu.disabled}
          onSelect={() => {
            // 更新状态已知时沿用带状态文案与重启动作的入口，否则退回普通的手动检查。
            if (updateMenu.visible) updateMenu.checkForUpdates();
            else execute(DesktopCommandIds.CheckForUpdates);
          }}
        >
          <RefreshCwIcon className="size-4" aria-hidden="true" />
          {intl.formatMessage(
            {
              id: updateMenu.visible ? updateMenu.labelId : "titleBar.menu.help.checkForUpdates",
            },
            updateMenu.visible ? updateMenu.labelValues : undefined,
          )}
        </DropdownMenuItem>
        <DropdownMenuItem
          data-testid="workspace-help-menu-about"
          onSelect={() => execute(DesktopCommandIds.ShowAbout)}
        >
          <InfoIcon className="size-4" aria-hidden="true" />
          {intl.formatMessage({ id: "titleBar.menu.help.about" })}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
