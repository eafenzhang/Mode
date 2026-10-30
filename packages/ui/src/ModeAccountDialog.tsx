import { useEffect, useState } from "react";
import { UserRound } from "lucide-react";
import type { CloudAccountState, UserInfo } from "@mode/shared";
import { BotsDialog } from "@/BotsDialog.js";
import { Button } from "@/ui-foundation/adapters/button.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useServices } from "@/hooks/useServices.js";
import { useSettings } from "@/hooks/useSettingService.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { resolveSidebarProfile } from "@/lib/sidebarProfile.js";
import { logger } from "@/logger.js";
import { RemoteControlCloudSection } from "@/remote-desktop/RemoteControlCloudSection.js";

/**
 * 左下角 Mode 账号入口：绑定微信 Bot → Host 自动登录并同步 → 账号设备列表/配对。
 * 视觉沿用原版账号菜单的“头像 + 登录/退出”位置，具体云账号操作复用现有卡片。
 * 昵称只覆盖本机展示（AppSettings.userNickname），不修改 OAuth / 云端账号资料。
 */
export function ModeAccountDialog({
  open,
  onOpenChange,
  workspacePath,
  workspaceIdentity,
  user,
  cloudAccount,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspacePath?: string;
  workspaceIdentity?: string;
  user?: UserInfo | null;
  cloudAccount?: CloudAccountState | null;
}) {
  const { intl } = useModeIntl();
  const services = useServices();
  const baseServices = useOptionalBaseWorkspaceServices();
  const service = baseServices?.cloudAccountService ?? services.cloudAccountService;
  const { settings, update } = useSettings();
  const [botsOpen, setBotsOpen] = useState(false);
  const [nicknameDraft, setNicknameDraft] = useState("");
  const [nicknameSaving, setNicknameSaving] = useState(false);
  const [nicknameSaved, setNicknameSaved] = useState(false);
  // footer 的云账号状态只在挂载/本进程事件时刷新；弹窗打开时补一次实时读取，
  // 避免“云端已经绑定、昵称区仍提示未登录”（跨进程写入时 onDidChange 不会到当前窗口）。
  const [liveCloudAccount, setLiveCloudAccount] = useState<CloudAccountState | null>(
    cloudAccount ?? null,
  );
  useEffect(() => {
    if (!open || !service) {
      return;
    }
    let cancelled = false;
    void service
      .getState()
      .then((next) => {
        if (!cancelled) {
          setLiveCloudAccount(next);
        }
      })
      .catch(() => undefined);
    const subscription = service.onDidChange((next) => {
      if (!cancelled) {
        setLiveCloudAccount(next);
      }
    });
    return () => {
      cancelled = true;
      subscription.dispose();
    };
  }, [open, service]);
  const effectiveCloudAccount = liveCloudAccount ?? cloudAccount ?? null;
  // 未部署 Mode Cloud（或当前 Host 没有该频道）时只提示部署缺失；
  // 云账号登录（GitHub/用户名密码）入口在远程控制页的 Mode Cloud 分区。
  const cloudUnavailable = !service || effectiveCloudAccount?.status === "unavailable";
  const descriptionKey = cloudUnavailable
    ? "modeAccount.description.unavailable"
    : "modeAccount.description";
  const bindHintKey = cloudUnavailable
    ? "modeAccount.bindHint.unavailable"
    : "modeAccount.bindHint";
  const profile = resolveSidebarProfile({
    user,
    cloudAccount: effectiveCloudAccount,
    nickname: settings?.userNickname,
  });
  const savedNickname = settings?.userNickname?.trim() ?? "";
  const message = (id: string): string => intl.formatMessage({ id });

  useEffect(() => {
    if (!open) {
      return;
    }
    setNicknameDraft(savedNickname);
    setNicknameSaved(false);
  }, [open, savedNickname]);

  const saveNickname = async (): Promise<void> => {
    setNicknameSaving(true);
    try {
      await update({ userNickname: nicknameDraft.trim() });
      setNicknameSaved(true);
    } catch (error) {
      // 写入失败时保留输入内容，由设置服务自己的日志/提示兜底；不能把「已保存」误报给用户。
      logger.error("[ModeAccountDialog] 保存昵称失败", { error });
    } finally {
      setNicknameSaving(false);
    }
  };
  const nicknameDirty = nicknameDraft.trim() !== savedNickname;
  const nicknameActionLabel = nicknameSaving
    ? message("modeAccount.nickname.saving")
    : nicknameSaved && !nicknameDirty
      ? message("modeAccount.nickname.saved")
      : message("modeAccount.nickname.save");

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
            <DialogDescription>
              {message(descriptionKey)}
            </DialogDescription>
          </DialogHeader>
          <section className="rounded-xl border border-border bg-card px-4 py-3">
            <h2 className="text-ui-lg font-medium text-foreground">
              {message("modeAccount.nickname.title")}
            </h2>
            <p className="mt-1 text-ui-base text-foreground-subtle">
              {message("modeAccount.nickname.description")}
            </p>
            {profile.hasAccount ? (
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <Input
                  value={nicknameDraft}
                  maxLength={32}
                  className="w-56"
                  aria-label={message("modeAccount.nickname.title")}
                  placeholder={message("modeAccount.nickname.placeholder")}
                  data-testid="mode-account-nickname-input"
                  onChange={(event) => {
                    setNicknameDraft(event.target.value);
                    setNicknameSaved(false);
                  }}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  data-testid="mode-account-nickname-save"
                  disabled={nicknameSaving || !nicknameDirty}
                  onClick={() => void saveNickname()}
                >
                  {nicknameActionLabel}
                </Button>
              </div>
            ) : (
              <p className="mt-2 text-ui-base text-foreground-subtle">
                {message("modeAccount.nickname.requiresAccount")}
              </p>
            )}
          </section>
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
              <p className="text-ui-xs text-foreground-subtle">
                {message(bindHintKey)}
              </p>
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
