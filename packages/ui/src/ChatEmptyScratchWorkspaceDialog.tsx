/**
 * 空态「创建工作区」对话框。
 *
 * 产品规则见 `docs/specs/workspace-creation.md`：
 * - 唯一写入者是 `IFileService.createScratchWorkspace`，这里只提交名称；
 * - 位置提示与 service 的 `~/ModeProject` 口径保持一致，只用于展示；
 * - 入口由空态工作区菜单在具备创建能力时渲染，本组件不自己决定可见性。
 */
import { useEffect, useState } from "react";
import { Button } from "@/ui-foundation/adapters/button.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { FolderIcon, LoaderIcon } from "lucide-react";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import {
  TID_COMPOSER_CREATE_WORKSPACE_CONFIRM,
  TID_COMPOSER_CREATE_WORKSPACE_DIALOG,
  TID_COMPOSER_CREATE_WORKSPACE_INPUT,
} from "@mode/shared";

export type ScratchWorkspaceNameErrorKind = "required" | "separator";

export function getScratchWorkspaceLocationHint(name: string) {
  return `~/ModeProject/${name.trim()}`;
}

export function getScratchWorkspaceNameErrorKind(
  name: string,
): ScratchWorkspaceNameErrorKind | null {
  const trimmedName = name.trim();
  if (!trimmedName) {
    return "required";
  }

  if (/[\\/]/.test(trimmedName)) {
    return "separator";
  }

  return null;
}

type SubmitErrorKind = ScratchWorkspaceNameErrorKind | "createFailed";

const ERROR_MESSAGE_ID: Record<SubmitErrorKind, string> = {
  required: "chat.empty.createWorkspace.error.required",
  separator: "chat.empty.createWorkspace.error.separator",
  createFailed: "chat.empty.createWorkspace.error.createFailed",
};

export interface ChatEmptyScratchWorkspaceDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 成功时返回新工作区路径；当前模式不支持时返回 `null`。 */
  onCreate: (name: string) => Promise<string | null>;
}

export function ChatEmptyScratchWorkspaceDialog({
  open,
  onOpenChange,
  onCreate,
}: ChatEmptyScratchWorkspaceDialogProps) {
  const { intl } = useModeIntl();
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [errorKind, setErrorKind] = useState<SubmitErrorKind | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    setName("");
    setSubmitting(false);
    setErrorKind(null);
  }, [open]);

  const trimmedName = name.trim();
  const locationPath = getScratchWorkspaceLocationHint(name);

  const handleOpenChange = (nextOpen: boolean) => {
    // 提交期间忽略关闭请求，避免目录已创建但对话框提前消失。
    if (!nextOpen && submitting) {
      return;
    }
    onOpenChange(nextOpen);
  };

  const handleSubmit = async () => {
    if (submitting) {
      return;
    }

    const validationError = getScratchWorkspaceNameErrorKind(name);
    if (validationError) {
      setErrorKind(validationError);
      return;
    }

    setSubmitting(true);
    setErrorKind(null);
    try {
      const createdPath = await onCreate(trimmedName);
      if (createdPath === null) {
        setErrorKind("createFailed");
        return;
      }
      onOpenChange(false);
    } catch (error) {
      logger.error("[ChatEmptyScratchWorkspaceDialog] 创建工作区失败", { error });
      setErrorKind("createFailed");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="max-w-md gap-0 overflow-hidden rounded-2xl p-0"
        data-testid={TID_COMPOSER_CREATE_WORKSPACE_DIALOG}
      >
        <DialogHeader className="gap-2 px-6 py-5 pb-0">
          <DialogTitle className="text-lg font-medium text-foreground">
            {intl.formatMessage({ id: "chat.empty.createWorkspace.title" })}
          </DialogTitle>
          <DialogDescription className="text-ui-base leading-6 text-foreground-subtle">
            {intl.formatMessage({ id: "chat.empty.createWorkspace.description" })}
          </DialogDescription>
        </DialogHeader>

        <form
          className="space-y-5 px-6 py-6"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSubmit();
          }}
        >
          <div className="space-y-2">
            <label
              htmlFor="chat-empty-create-workspace-input"
              className="inline-flex text-ui-base font-medium text-foreground-subtle"
            >
              {intl.formatMessage({ id: "chat.empty.createWorkspace.nameLabel" })}
            </label>
            <Input
              id="chat-empty-create-workspace-input"
              data-testid={TID_COMPOSER_CREATE_WORKSPACE_INPUT}
              size="lg"
              autoFocus
              value={name}
              disabled={submitting}
              invalid={errorKind !== null}
              placeholder={intl.formatMessage({ id: "chat.empty.createWorkspace.placeholder" })}
              className="h-10 rounded-lg bg-background/50"
              onChange={(event) => {
                setName(event.target.value);
                setErrorKind(null);
              }}
            />
          </div>

          <div className="space-y-2">
            <span className="inline-flex text-ui-base font-medium text-foreground-subtle">
              {intl.formatMessage({ id: "chat.empty.createWorkspace.locationLabel" })}
            </span>
            <div className="flex min-w-0 items-center gap-2 rounded-lg border border-dashed border-border bg-background/40 px-3 py-2.5">
              <FolderIcon className="size-4 shrink-0 text-foreground-subtle" />
              <span dir="ltr" className="min-w-0 flex-1 truncate text-ui-base text-foreground">
                {intl.formatMessage(
                  { id: "chat.empty.createWorkspace.locationHint" },
                  { path: locationPath },
                )}
              </span>
            </div>
            <p className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "chat.empty.createWorkspace.locationDescription" })}
            </p>
          </div>

          {errorKind !== null ? (
            <p role="alert" className="text-ui-base text-destructive">
              {intl.formatMessage({ id: ERROR_MESSAGE_ID[errorKind] })}
            </p>
          ) : null}

          <DialogFooter className="gap-2 pt-4">
            <Button
              type="button"
              variant="secondary"
              size="lg"
              onClick={() => handleOpenChange(false)}
              disabled={submitting}
              className="h-10 min-w-0 px-5"
            >
              {intl.formatMessage({ id: "common.cancel" })}
            </Button>
            <Button
              type="submit"
              size="lg"
              disabled={submitting}
              data-testid={TID_COMPOSER_CREATE_WORKSPACE_CONFIRM}
              className="h-10 min-w-0 px-5"
            >
              {submitting ? <LoaderIcon className="size-4 animate-spin" /> : null}
              {intl.formatMessage({ id: "chat.empty.createWorkspace.confirm" })}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
