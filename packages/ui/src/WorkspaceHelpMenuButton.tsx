import { useEffect, useState } from "react";
import {
  DesktopCommandIds,
  TID_WORKSPACE_HELP_MENU_TRIGGER,
  type DesktopCommandId,
} from "@mode/shared";
import {
  CircleHelpIcon,
  FileDownIcon,
  InfoIcon,
  RefreshCwIcon,
  SquareLibraryIcon,
} from "lucide-react";
import { Button } from "@/ui-foundation/adapters/button.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/ui-foundation/adapters/dropdown-menu.js";
import { cn } from "@/components/lib/utils.js";
import { ControlHintTooltip } from "@/ControlHintTooltip.js";
import { runUpdateCheck } from "@/desktopUpdateFeedback.js";
import { dismissToast, toast } from "@/ui-foundation/adapters/toast.js";
import { useDesktopUpdateMenu } from "@/hooks/useDesktopUpdateMenu.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 右上角「帮助」入口：资源管理器、检查更新、导出日志、关于 Mode。
 *
 * 社区 / 问题反馈 / 功能建议 / 开发者工具 / 清除数据等入口已下线（程序更新由静默更新承担，
 * 手动检查更新只作为明确入口保留）。**导出日志是 2026-10-26 恢复的入口**：壳命令已实现
 * （spec `tauri-platform-commands.md` 第 15 行），此前拒绝桩时代把入口一起下线了。
 * 菜单标题与条目文案复用既有 i18n 键（成功提示新增 `sidebar.exportLogs.success`）。
 * Web/fpk 没有资源管理器与桌面更新器：只保留「导出日志」与「关于」，前者走服务端
 * 日志包下载，后者用本组件的只读对话框。
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
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const updateMenu = useDesktopUpdateMenu(isDesktop);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [aboutVersion, setAboutVersion] = useState<string | null>(null);
  useEffect(() => {
    if (!aboutOpen || aboutVersion !== null) {
      return;
    }
    void platform.getAppVersion?.().then(
      (version) => setAboutVersion(version),
      () => setAboutVersion("unknown"),
    );
  }, [aboutOpen, aboutVersion, platform]);
  const menuLabel = intl.formatMessage({ id: "workspaceHeader.help.menu" });
  const execute = (command: DesktopCommandId) => {
    void platform.executeDesktopCommand(command);
  };
  const formatMessage = (id: string, values?: Record<string, string | number>) =>
    intl.formatMessage({ id }, values);
  const checkForUpdates = () =>
    runUpdateCheck({
      execute: () => platform.executeDesktopCommand(DesktopCommandIds.CheckForUpdates),
      formatMessage,
      showToast: (message, options) => toast(message, options),
    });
  /**
   * 导出日志：壳命令把 Host 文件日志打成一个文本包并经保存对话框落盘（spec 第 15 行）。
   *
   * 三种结局分开处理：成功 → 给出落盘路径；**用户取消（success:false 且无 error）不弹错**；
   * 真失败才提示 error 码。pending toast 常驻到结局再收起，避免失败时误留一条「正在导出」。
   */
  const exportLogs = async () => {
    const pendingId = toast(formatMessage("sidebar.exportLogs.pending"), {
      durationMs: Number.POSITIVE_INFINITY,
    });
    try {
      const result = await platform.exportLogs();
      if (result.success) {
        toast(formatMessage("sidebar.exportLogs.success", { path: result.path ?? "" }));
      } else if (result.error) {
        toast(formatMessage("sidebar.exportLogs.error", { error: result.error }));
      }
    } catch (error) {
      toast(
        formatMessage("sidebar.exportLogs.error", {
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      dismissToast(pendingId);
    }
  };

  return (
    <>
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
          {isDesktop && platform.supportsResourceManager !== false ? (
            <DropdownMenuItem
              data-testid="workspace-help-menu-resource-manager"
              onSelect={() => execute(DesktopCommandIds.OpenResourceManager)}
            >
              <SquareLibraryIcon className="size-4" aria-hidden="true" />
              {intl.formatMessage({ id: "titleBar.menu.help.resourceManager" })}
            </DropdownMenuItem>
          ) : null}
          {isDesktop ? (
            <DropdownMenuItem
              data-testid="workspace-help-menu-check-for-updates"
              disabled={updateMenu.disabled}
              onSelect={() => {
                // 菜单文案仍跟随更新状态；点击本身统一走一次检查并消费壳返回值。
                void checkForUpdates();
              }}
            >
              <RefreshCwIcon className="size-4" aria-hidden="true" />
              {intl.formatMessage(
                {
                  id: updateMenu.visible
                    ? updateMenu.labelId
                    : "titleBar.menu.help.checkForUpdates",
                },
                updateMenu.visible ? updateMenu.labelValues : undefined,
              )}
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            data-testid="workspace-help-menu-export-logs"
            onSelect={() => {
              void exportLogs();
            }}
          >
            <FileDownIcon className="size-4" aria-hidden="true" />
            {intl.formatMessage({ id: "titleBar.menu.help.exportLogs" })}
          </DropdownMenuItem>
          <DropdownMenuItem
            data-testid="workspace-help-menu-about"
            onSelect={() => {
              if (isDesktop) {
                execute(DesktopCommandIds.ShowAbout);
                return;
              }
              setAboutOpen(true);
            }}
          >
            <InfoIcon className="size-4" aria-hidden="true" />
            {intl.formatMessage({ id: "titleBar.menu.help.about" })}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={aboutOpen} onOpenChange={setAboutOpen}>
        <DialogContent className="max-w-sm gap-0 overflow-hidden rounded-2xl p-0">
          <DialogHeader className="gap-2 px-6 py-5 pb-0">
            <DialogTitle className="text-lg font-medium text-foreground">
              {intl.formatMessage({ id: "titleBar.menu.help.about" })}
            </DialogTitle>
            <DialogDescription className="text-ui-base leading-6 text-foreground-subtle">
              {intl.formatMessage(
                { id: "settings.about.version" },
                { version: aboutVersion ?? "…" },
              )}
            </DialogDescription>
          </DialogHeader>
          <p className="px-6 py-5 text-ui-base leading-6 text-foreground-subtle">
            {intl.formatMessage({ id: "settings.about.description" })}
          </p>
          <DialogFooter className="gap-2 px-6 pb-5">
            <Button
              type="button"
              variant="secondary"
              onClick={() => {
                void platform.openExternal("https://github.com/eafenzhang/Mode");
              }}
            >
              {intl.formatMessage({ id: "settings.about.project" })}
            </Button>
            <Button type="button" onClick={() => setAboutOpen(false)}>
              {intl.formatMessage({ id: "common.close" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
