import { useEffect, useState, useRef } from "react";
import { createPortal } from "react-dom";
import type { IServiceAccessor } from "@mode/services";
import type { FileEntry } from "@mode/shared";
import { FileIcon, FolderIcon, FolderSymlinkIcon } from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { Button } from "./components/ui/button.js";
import { Input } from "./components/ui/input.js";
import { useModeIntl } from "./i18n/IntlProvider.js";
import { logger } from "./logger.js";

function createDirectoryBrowserReadParams(
  path: string,
  showHiddenDirectories: boolean,
): { path: string; includeHidden?: boolean } {
  return showHiddenDirectories ? { path, includeHidden: true } : { path };
}

function filterDirectoryBrowserEntries(entries: FileEntry[]): FileEntry[] {
  return entries.filter((e) => e.type === "directory");
}

// 目录模式只暴露目录（既有 web 远端建连选路径行为）；file 模式需要同时展示目录与文件条目，供多选附件。
export function selectDirectoryBrowserEntries(
  entries: FileEntry[],
  mode: "directory" | "file",
): FileEntry[] {
  return mode === "file" ? entries : filterDirectoryBrowserEntries(entries);
}

export interface DirectoryBrowserInteraction {
  /** footer 主动作：只浏览只给关闭，file 给多选确认，directory 给选当前目录。 */
  footer: "close" | "selectFiles" | "selectDir";
  /** 文件条目是否参与选中；只浏览下文件只是展示项。 */
  filesSelectable: boolean;
}

/** footer 与文件可选性由 mode + browseOnly 唯一决定，抽成纯函数便于单测钉住只浏览语义。 */
export function directoryBrowserInteraction(
  mode: "directory" | "file",
  browseOnly: boolean,
): DirectoryBrowserInteraction {
  if (browseOnly) return { footer: "close", filesSelectable: false };
  return mode === "file"
    ? { footer: "selectFiles", filesSelectable: true }
    : { footer: "selectDir", filesSelectable: false };
}

// 远程目录条目已经携带 isSymbolicLink，这里只替换展示图标，不改变进入和选择目录的路径语义。
function getDirectoryBrowserEntryIconKind(
  entry: Pick<FileEntry, "isSymbolicLink" | "type">,
): "folder" | "folder-symlink" {
  return entry.type === "directory" && entry.isSymbolicLink === true ? "folder-symlink" : "folder";
}

function createDirectoryBrowserRequestGuard(): {
  begin: () => number;
  isCurrent: (requestId: number) => boolean;
} {
  let latestRequestId = 0;

  return {
    begin: () => {
      latestRequestId += 1;
      return latestRequestId;
    },
    isCurrent: (requestId: number) => requestId === latestRequestId,
  };
}

/**
 * 服务端目录浏览对话框。
 * 通过 fileService.readdir 浏览远程/本地服务器的目录结构，
 * 适用于 web 端无法调用系统原生文件选择器的场景。
 */
export function DirectoryBrowser({
  services,
  onSelect,
  onCancel,
  onPathChange,
  embedded = false,
  mode = "directory",
  initialPath,
  onSelectFiles,
  browseOnly = false,
}: {
  services: IServiceAccessor;
  onSelect: (path: string) => void;
  onCancel: () => void;
  onPathChange?: (path: string) => void;
  embedded?: boolean;
  /** "directory"（默认）只选当前目录；"file" 列出文件并支持多选附件。 */
  mode?: "directory" | "file";
  /** file 模式起点；缺省 homedir。 */
  initialPath?: string;
  /** file 模式多选确认回调。 */
  onSelectFiles?: (paths: string[]) => void;
  /**
   * 只浏览（文件管理器语义）：列目录与文件、可导航，但不选中、不回填，footer 只有关闭。
   * 多选回填仍只属于 composer 的「从工作区选择文件」，两条入口不合并。
   */
  browseOnly?: boolean;
}) {
  const [currentPath, setCurrentPath] = useState("");
  const [inputPath, setInputPath] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showHiddenDirectories, setShowHiddenDirectories] = useState(false);
  const [selectedPaths, setSelectedPaths] = useState<ReadonlySet<string>>(() => new Set());
  const listRef = useRef<HTMLDivElement>(null);
  const showHiddenDirectoriesRef = useRef(false);
  const requestGuardRef = useRef(createDirectoryBrowserRequestGuard());
  const { intl } = useModeIntl();
  const interaction = directoryBrowserInteraction(mode, browseOnly);

  // 初始化：获取 homedir 作为起始路径；有 initialPath 时以其为起点
  useEffect(() => {
    services.systemService
      .info()
      .then((info) => {
        navigateTo(initialPath?.trim() || info.homedir);
      })
      .catch((err) => {
        // 远端 systemService 抖动不阻断浏览：只要给了 initialPath 就直接尝试导航，
        // 读不到系统信息时仍可进入起点目录；否则保持原错误提示路径。
        const fallbackPath = initialPath?.trim();
        if (fallbackPath) {
          navigateTo(fallbackPath);
          return;
        }
        logger.error("[DirectoryBrowser] 获取系统信息失败:", err);
        setError(intl.formatMessage({ id: "directoryBrowser.errorSystem" }));
        setLoading(false);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const navigateTo = async (
    path: string,
    nextShowHiddenDirectories = showHiddenDirectoriesRef.current,
  ) => {
    const requestId = requestGuardRef.current.begin();
    setLoading(true);
    setError("");
    try {
      const result = await services.fileService.readdir(
        createDirectoryBrowserReadParams(path, nextShowHiddenDirectories),
      );
      if (!requestGuardRef.current.isCurrent(requestId)) {
        return;
      }
      // mode 在组件生命周期内不变化（初始化 effect 以 [] 注册），闭包直接引用即可。
      setEntries(selectDirectoryBrowserEntries(result, mode));
      setCurrentPath(path);
      setInputPath(path);
      onPathChange?.(path);
      // 滚动到顶部
      listRef.current?.scrollTo(0, 0);
    } catch (err) {
      if (!requestGuardRef.current.isCurrent(requestId)) {
        return;
      }
      // 用户快速切换目录或隐藏目录时，较早 readdir 可能后返回；
      // 过期错误不能覆盖最新导航状态，否则 UI 会展示和当前按钮状态不一致的目录/错误。
      logger.error("[DirectoryBrowser] readdir 失败:", err);
      setError(intl.formatMessage({ id: "directoryBrowser.errorReadDir" }, { error: String(err) }));
    } finally {
      if (requestGuardRef.current.isCurrent(requestId)) {
        setLoading(false);
      }
    }
  };

  const handleGoUp = () => {
    // 提取父目录路径
    const parent = currentPath.replace(/\/[^/]+\/?$/, "") || "/";
    navigateTo(parent);
  };

  const handleInputSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = inputPath.trim();
    if (trimmed) navigateTo(trimmed);
  };

  const handleToggleHiddenDirectories = () => {
    const nextShowHiddenDirectories = !showHiddenDirectoriesRef.current;
    showHiddenDirectoriesRef.current = nextShowHiddenDirectories;
    setShowHiddenDirectories(nextShowHiddenDirectories);

    if (currentPath) {
      void navigateTo(currentPath, nextShowHiddenDirectories);
    }
  };

  const handleSelectCurrentPath = () => {
    const selectedPath = currentPath.trim();
    if (!selectedPath || loading) {
      return;
    }

    onSelect(selectedPath);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onCancel();
  };

  const browserContent = (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div>
        <div className="mb-1 block text-ui-base text-foreground-subtle">
          {intl.formatMessage({ id: "directoryBrowser.title" })}
        </div>

        {/* 路径输入 */}
        <form onSubmit={handleInputSubmit} className="flex flex-col gap-2 sm:flex-row">
          <Input
            type="text"
            size="lg"
            value={inputPath}
            onChange={(e) => setInputPath(e.target.value)}
            placeholder={intl.formatMessage({
              id: "directoryBrowser.pathPlaceholder",
            })}
            className="h-9 min-w-0 flex-1 text-ui-base"
            autoFocus
          />
          <div className="flex gap-2 sm:shrink-0">
            <Button
              type="submit"
              size="lg"
              className="h-9 min-w-0 flex-1 px-4 text-ui-base sm:flex-none"
            >
              {intl.formatMessage({ id: "directoryBrowser.go" })}
            </Button>
            <Button
              type="button"
              variant={showHiddenDirectories ? "secondary" : "outline"}
              size="lg"
              aria-pressed={showHiddenDirectories}
              className="h-9 min-w-0 flex-1 px-3 text-ui-base sm:flex-none"
              onClick={handleToggleHiddenDirectories}
            >
              {intl.formatMessage({
                id: showHiddenDirectories
                  ? "directoryBrowser.hideHidden"
                  : "directoryBrowser.showHidden",
              })}
            </Button>
          </div>
        </form>
      </div>

      {/* 错误提示 */}
      {error ? <div className="text-ui-base text-destructive">{error}</div> : null}

      {/* 目录列表 */}
      <div
        ref={listRef}
        className="flex-1 flex flex-col overflow-y-auto rounded-lg border border-border bg-background"
      >
        {/* 上级目录 */}
        {currentPath && currentPath !== "/" ? (
          <button
            onClick={handleGoUp}
            // 滚动容器是纵向 flex 布局，子项默认允许 shrink；
            // 当目录很多触发滚动时，首行“返回上级”会被压缩导致高度变化。
            className="flex h-10 w-full shrink-0 cursor-pointer items-center gap-2 border-b border-border px-3 text-start text-ui-base transition hover:bg-hover/50"
          >
            <FolderIcon className="size-4 shrink-0 text-foreground-subtle" />
            <span className="text-foreground">..</span>
          </button>
        ) : null}

        {loading ? (
          <div className="p-4 text-center text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "common.loading" })}
          </div>
        ) : entries.length === 0 ? (
          <div className="p-4 text-center text-ui-base text-foreground-subtle">
            {intl.formatMessage({
              id: mode === "file" ? "directoryBrowser.noEntries" : "directoryBrowser.noSubdirs",
            })}
          </div>
        ) : (
          entries.map((entry) => {
            if (mode === "file" && entry.type === "file") {
              // 只浏览形态下文件只是展示项：不参与选中，也没有 aria-pressed/选中态可言。
              if (!interaction.filesSelectable) {
                return (
                  <div
                    key={entry.path}
                    className="flex w-full shrink-0 items-center gap-2 px-3 py-2 text-start text-ui-base"
                  >
                    <FileIcon className="size-4 shrink-0 text-foreground-subtle" />
                    <span className="truncate">{entry.name}</span>
                  </div>
                );
              }
              const selected = selectedPaths.has(entry.path);
              return (
                <button
                  key={entry.path}
                  type="button"
                  aria-pressed={selected}
                  data-selected={selected || undefined}
                  onClick={() =>
                    setSelectedPaths((current) => {
                      const next = new Set(current);
                      if (next.has(entry.path)) next.delete(entry.path);
                      else next.add(entry.path);
                      return next;
                    })
                  }
                  className={cn(
                    "flex w-full shrink-0 cursor-pointer items-center gap-2 px-3 py-2 text-start text-ui-base transition hover:bg-hover/50",
                    selected && "bg-accent/60",
                  )}
                >
                  <FileIcon className="size-4 shrink-0 text-foreground-subtle" />
                  <span className="truncate">{entry.name}</span>
                </button>
              );
            }
            const EntryIcon =
              getDirectoryBrowserEntryIconKind(entry) === "folder-symlink"
                ? FolderSymlinkIcon
                : FolderIcon;
            return (
              <button
                key={entry.path}
                onClick={() => navigateTo(entry.path)}
                className="flex w-full shrink-0 cursor-pointer items-center gap-2 px-3 py-2 text-start text-ui-base transition hover:bg-hover/50"
              >
                <EntryIcon className="size-4 shrink-0 text-foreground-subtle" />
                <span className="truncate">{entry.name}</span>
              </button>
            );
          })
        )}
      </div>
    </div>
  );

  if (embedded) {
    return browserContent;
  }

  // 非嵌入 overlay 必须 portal 到 body：composer 把本组件挂在
  // ConversationBottomDockTransition 的 motion.div 内联 transform 之下，transform 会成为
  // fixed 后代的包含块，弹窗因此只在底部输入栏那一小条盒子里居中、贴着屏幕下方溢出。
  // portal 到 body 后 fixed 以视口为包含块，才能真正屏幕居中（embedded 形态原位返回，不受影响）。
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-foreground/30 backdrop-blur-sm"
      onClick={(e) => e.target === e.currentTarget && onCancel()}
      onKeyDown={handleKeyDown}
    >
      <div className="flex h-[min(36rem,calc(100vh-4rem))] w-[min(44rem,calc(100vw-2rem))] flex-col rounded-xl border border-border bg-popover p-4 shadow-xl">
        {browserContent}
        {/* 非嵌入模式之前只能继续进入子目录，没有确认当前目录的路径。
            Web 目录浏览器作为独立选择器使用时因此永远拿不到 onSelect 结果。
            这里把确认动作收口到弹窗 footer，选择当前浏览路径并交回上层。
            browseOnly（文件管理器）没有可回填的选择语义，footer 只留关闭。 */}
        <div className="mt-3 flex shrink-0 justify-end gap-2">
          {interaction.footer === "close" ? (
            <Button
              type="button"
              size="lg"
              className="h-10 min-w-0 px-5 text-ui-base"
              onClick={onCancel}
            >
              {intl.formatMessage({ id: "common.close" })}
            </Button>
          ) : (
            <>
              <Button
                type="button"
                variant="secondary"
                size="lg"
                className="h-10 min-w-0 px-5 text-ui-base"
                onClick={onCancel}
              >
                {intl.formatMessage({ id: "common.cancel" })}
              </Button>
              {interaction.footer === "selectFiles" ? (
                <Button
                  type="button"
                  size="lg"
                  className="h-10 min-w-0 px-5 text-ui-base"
                  disabled={selectedPaths.size === 0 || loading}
                  onClick={() => onSelectFiles?.([...selectedPaths])}
                >
                  {intl.formatMessage(
                    { id: "directoryBrowser.selectFiles" },
                    { count: String(selectedPaths.size) },
                  )}
                </Button>
              ) : (
                <Button
                  type="button"
                  size="lg"
                  className="h-10 min-w-0 px-5 text-ui-base"
                  disabled={!currentPath.trim() || loading}
                  onClick={handleSelectCurrentPath}
                >
                  {intl.formatMessage({ id: "directoryBrowser.selectDir" })}
                </Button>
              )}
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
