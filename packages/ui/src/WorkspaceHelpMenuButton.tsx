import { DesktopCommandIds, TID_WORKSPACE_HELP_MENU_TRIGGER } from "@zcode/shared";
import { SquareLibraryIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { cn } from "@/components/lib/utils.js";
import { ControlHintTooltip } from "@/ControlHintTooltip.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";

/**
 * 右上角入口 = 资源管理器（原「帮助」菜单已下线：社区 / 问题反馈 / 功能建议 / 关于等入口全部删除，
 * 程序更新由静默更新承担，不再提供手动入口）。Web 端没有资源管理器窗口，不渲染。
 */
export function WorkspaceHelpMenuButton({
  className,
  isDesktop = false,
}: {
  className?: string;
  /**
   * 是否桌面端。由挂载处注入而不是在组件内嗅探：Web 的 IPlatformService 桩同样实现了
   * executeDesktopCommand（no-op），拿它判定会让 Web 端出现一个点了没反应的「资源管理器」。
   */
  isDesktop?: boolean;
}) {
  const { intl } = useZCodeIntl();
  const platform = usePlatform();
  if (!isDesktop) {
    return null;
  }
  const label = intl.formatMessage({ id: "titleBar.menu.help.resourceManager" });

  return (
    <ControlHintTooltip title={label} side="bottom">
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
        aria-label={label}
        data-testid={TID_WORKSPACE_HELP_MENU_TRIGGER}
        onClick={() => void platform.executeDesktopCommand(DesktopCommandIds.OpenResourceManager)}
      >
        <SquareLibraryIcon className="size-4" />
      </Button>
    </ControlHintTooltip>
  );
}
