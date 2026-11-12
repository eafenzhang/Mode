import { useEffect, useState } from "react";
import type { RemoteV3HostState, UserInfo } from "@mode/shared";
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
import { RemoteV3AccountSection } from "@/account/RemoteV3AccountSection.js";

/**
 * 左下角 Mode 账号入口：远程控制 v3 账号登录、设备目录与本机昵称。
 * 昵称只覆盖本机展示（AppSettings.userNickname），不修改账号资料。
 */
export function ModeAccountDialog({
  open,
  onOpenChange,
  user,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  user?: UserInfo | null;
}) {
  const { intl } = useModeIntl();
  const services = useServices();
  const baseServices = useOptionalBaseWorkspaceServices();
  const remoteV3Service = baseServices?.remoteV3Service ?? services.remoteV3Service;
  const { settings, update } = useSettings();
  const [nicknameDraft, setNicknameDraft] = useState("");
  const [nicknameSaving, setNicknameSaving] = useState(false);
  const [nicknameSaved, setNicknameSaved] = useState(false);
  const [remoteV3State, setRemoteV3State] = useState<RemoteV3HostState | null>(null);
  useEffect(() => {
    if (!open || !remoteV3Service) return;
    let cancelled = false;
    void remoteV3Service
      .getState()
      .then((next) => {
        if (!cancelled) setRemoteV3State(next);
      })
      .catch(() => undefined);
    const subscription = remoteV3Service.onDidChange((next) => {
      if (!cancelled) setRemoteV3State(next);
    });
    return () => {
      cancelled = true;
      subscription.dispose();
    };
  }, [open, remoteV3Service]);
  const profile = resolveSidebarProfile({
    user,
    remoteV3: remoteV3State,
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
            <DialogDescription>{message("modeAccount.description")}</DialogDescription>
          </DialogHeader>
          <section className="rounded-xl border border-border bg-card px-4 py-3">
            <h2 className="text-ui-lg font-medium text-foreground">
              {message("modeAccount.nickname.title")}
            </h2>
            <p className="mt-1 text-ui-base text-foreground-subtle">
              {message("modeAccount.nickname.description")}
            </p>
            {profile.hasAccount || remoteV3State?.status === "online" ? (
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
          <RemoteV3AccountSection active={open} />
        </DialogContent>
      </Dialog>
    </>
  );
}
