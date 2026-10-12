import { TID_WORKSPACE_FILE_MANAGER_BUTTON, type RemoteTarget } from "@mode/shared";
import { useState } from "react";
import { FolderOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { DirectoryBrowser } from "@/DirectoryBrowser.js";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { shouldShowWorkspaceFileManager } from "@/lib/workspaceFileManagerEntry.js";

/**
 * 远端工作区右上角的文件管理器入口（spec: remote-workspace-file-manager-entry）。
 *
 * 打开内置 DirectoryBrowser 的只浏览形态，起点为当前工作区路径。远端拿不到系统文件管理器
 * （SSH/Docker/LAN 的路径在本机不存在），所以这里不走 platform.openInFileManager；
 * 服务访问器复用 useWorkspaceServicesResolution 这一条既有解析路径，不新建连接。
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
  const [open, setOpen] = useState(false);
  const { services } = useWorkspaceServicesResolution(
    workspaceAbsPath,
    remoteSessionId,
    workspaceIdentity,
    remoteTarget,
  );

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
      {/* 与本机「打开方式」胶囊同款式（spec: remote-workspace-file-manager-entry 外观一致）：
          同款 h-7 容器 + 边框底色，内嵌 size-7 按钮；远端无打开目标可切换，故不带下拉。 */}
      <div className="flex items-center h-7 me-1.5 rounded-lg border border-border bg-input overflow-hidden p-0 hover:border-border-hover">
        <Button
          type="button"
          variant="ghost"
          size="icon-md"
          className="size-7 rounded-none border-0 text-foreground hover:bg-hover hover:text-foreground [app-region:no-drag]"
          aria-label={label}
          title={label}
          data-testid={TID_WORKSPACE_FILE_MANAGER_BUTTON}
          onClick={() => setOpen(true)}
        >
          <FolderOpenIcon className="size-4" aria-hidden="true" />
        </Button>
      </div>
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
