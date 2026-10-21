import { useState } from "react";
import { UserRound } from "lucide-react";
import { BotsDialog } from "@/BotsDialog.js";
import { Button } from "@/ui-foundation/adapters/button.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import { useServices } from "@/hooks/useServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { RemoteControlCloudSection } from "@/remote-desktop/RemoteControlCloudSection.js";

/**
 * 左下角 Mode 账号入口：绑定微信 Bot → Host 自动登录并同步 → 账号设备列表/配对。
 * 视觉沿用原版账号菜单的“头像 + 登录/退出”位置，具体云账号操作复用现有卡片。
 */
export function ModeAccountDialog({
  open,
  onOpenChange,
  workspacePath,
  workspaceIdentity,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspacePath?: string;
  workspaceIdentity?: string;
}) {
  const { intl } = useModeIntl();
  const services = useServices();
  const service = services.cloudAccountService;
  const [botsOpen, setBotsOpen] = useState(false);
  const message = (id: string): string => intl.formatMessage({ id });

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent
          // 修复原因（2026-10-21）：M4 Dialog 适配器本身不含内边距，账号弹窗的标题、
          // 说明和 Mode Cloud 卡片曾直接贴着圆角边缘；与 BotsDialog 一样显式补
          // p-4 + 纵向 gap，并给 header 预留右上角关闭按钮的位置。
          className="flex max-w-2xl flex-col gap-4 p-4"
          data-testid="mode-account-dialog"
        >
          <DialogHeader className="pe-10">
            <DialogTitle>{message("modeAccount.title")}</DialogTitle>
            <DialogDescription>{message("modeAccount.description")}</DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="mode-account-bind-bot"
                onClick={() => setBotsOpen(true)}
              >
                <UserRound className="size-4" />
                {message("modeAccount.bindBot")}
              </Button>
              <p className="text-ui-xs text-foreground-subtle">{message("modeAccount.bindHint")}</p>
            </div>
            {service ? (
              <RemoteControlCloudSection active={open} onPaired={() => undefined} />
            ) : (
              <p className="text-ui-base text-foreground-subtle">
                {message("modeAccount.unsupported")}
              </p>
            )}
          </div>
        </DialogContent>
      </Dialog>
      {botsOpen ? (
        <BotsDialog
          open={botsOpen}
          onOpenChange={setBotsOpen}
          workspacePath={workspacePath ?? ""}
          workspaceIdentity={workspaceIdentity}
          entryProvider="weixin"
        />
      ) : null}
    </>
  );
}
