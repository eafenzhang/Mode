import { TID_WORKSPACE_FILE_MANAGER_BUTTON, type RemoteTarget } from "@mode/shared";
import { useEffect, useState } from "react";
import { ChevronDown, FolderOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { DirectoryBrowser } from "@/DirectoryBrowser.js";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { isFileManagerOpenTarget, sortInstalledEditorsForOpenWith } from "@/lib/openWithEditors.js";
import { shouldShowWorkspaceFileManager } from "@/lib/workspaceFileManagerEntry.js";

/**
 * 远端工作区右上角的文件管理器入口（spec: remote-workspace-file-manager-entry）。
 *
 * 打开内置 DirectoryBrowser 的只浏览形态，起点为当前工作区路径。远端拿不到系统文件管理器
 * （SSH/Docker/LAN 的路径在本机不存在），所以这里不走 platform.openInFileManager；
 * 服务访问器复用 useWorkspaceServicesResolution 这一条既有解析路径，不新建连接。
 *
 * 外观 = 图 2 样式（与本机「打开方式」胶囊同款）：彩色文件夹图标 + 装饰性下拉箭头，
 * 整颗是一颗按钮——点击（含箭头）都打开浏览窗口，不弹下拉菜单（远端没有可切换的打开目标）。
 */
export function WorkspaceFileManagerButton({
  workspaceAbsPath,
  workspaceIdentity,
  remoteSessionId,
  remoteTarget,
  simplifyForNarrowRemote = false,
}: {
  workspaceAbsPath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
  remoteTarget?: RemoteTarget;
  simplifyForNarrowRemote?: boolean;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [open, setOpen] = useState(false);
  const [fileManagerIconUrl, setFileManagerIconUrl] = useState<string | null>(null);
  const { services } = useWorkspaceServicesResolution(
    workspaceAbsPath,
    remoteSessionId,
    workspaceIdentity,
    remoteTarget,
  );

  // 与本机胶囊同一把图标尺子：Windows 编辑器图标视觉占比更大，用 16px，其它平台 20px。
  const iconClassName =
    typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent)
      ? "size-4 shrink-0"
      : "size-5 shrink-0";

  useEffect(() => {
    let disposed = false;
    platform
      .getInstalledEditors()
      .then((editors) => {
        if (disposed) {
          return;
        }
        // 取本机文件管理器（finder/qspace/explorer）的彩色图标做视觉一致；拿不到就回退 lucide。
        const fileManager = sortInstalledEditorsForOpenWith(editors).find(isFileManagerOpenTarget);
        setFileManagerIconUrl(fileManager?.iconDataUrl ?? null);
      })
      .catch(() => {
        // 无图标只影响观感，入口本身不受影响。
      });
    return () => {
      disposed = true;
    };
  }, [platform]);

  if (
    !shouldShowWorkspaceFileManager({
      remoteTarget,
      workspaceIdentity,
      remoteSessionId,
      simplifyForNarrowRemote,
    })
  ) {
    return null;
  }

  const label = intl.formatMessage({ id: "workspaceHeader.fileManager" });

  return (
    <>
      {/* 与本机「打开方式」胶囊同款式容器（h-7 / border-border / bg-input / me-1.5），
          但整颗是一颗按钮：远端没有下拉菜单可弹，箭头只是图 2 外观的一部分。 */}
      <Button
        type="button"
        variant="ghost"
        size="default"
        className="me-1.5 h-7 gap-1 rounded-lg border border-border bg-input px-2 hover:border-border-hover [app-region:no-drag]"
        aria-label={label}
        title={label}
        data-testid={TID_WORKSPACE_FILE_MANAGER_BUTTON}
        onClick={() => setOpen(true)}
      >
        {fileManagerIconUrl ? (
          <img src={fileManagerIconUrl} alt="" className={iconClassName} />
        ) : (
          <FolderOpenIcon className="size-4" aria-hidden="true" />
        )}
        <ChevronDown className="size-3.5 shrink-0 text-foreground-subtlest" aria-hidden="true" />
      </Button>
      {open ? (
        <DirectoryBrowser
          services={services}
          mode="file"
          browseOnly
          initialPath={workspaceAbsPath}
          // browseOnly 永不回填；类型必传，这里等价占位关闭。
          onSelect={() => setOpen(false)}
          onCancel={() => setOpen(false)}
        />
      ) : null}
    </>
  );
}
