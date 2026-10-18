// M4 adapter：DiffViewer（旧 API：patch 或 oldFile/newFile 二选一）。
// 直接使用 @pierre/diffs 的公共 React 组件，只负责主题/字号/滚动容器。

import { memo, useMemo, type HTMLAttributes } from "react";
import type { BundledTheme } from "shiki";
import type { FileContents, FileDiffOptions } from "@pierre/diffs";
import { MultiFileDiff, PatchDiff } from "@pierre/diffs/react";

import {
  buildDiffViewerOptions,
  buildDiffViewerStyle,
  diffViewerContainerClassName,
} from "../diffViewerStyles.js";

export interface DiffViewerSelectedLineRange {
  start: number;
  end: number;
  side: "additions" | "deletions";
}

interface DiffViewerPatchInput {
  patch: string;
}

interface DiffViewerMultiFileInput {
  oldFile: FileContents;
  newFile: FileContents;
}

export type DiffViewerProps = Omit<HTMLAttributes<HTMLDivElement>, "children"> &
  (DiffViewerPatchInput | DiffViewerMultiFileInput) & {
    options?: FileDiffOptions<undefined>;
    disableWorkerPool?: boolean;
    diffClassName?: string;
    fontSizePx?: number;
    lightTheme?: BundledTheme;
    darkTheme?: BundledTheme;
    themeType?: "light" | "dark";
    selectedLines?: DiffViewerSelectedLineRange | null;
  };

function DiffViewerComponent(props: DiffViewerProps) {
  const {
    options,
    disableWorkerPool,
    diffClassName,
    fontSizePx,
    lightTheme,
    darkTheme,
    themeType,
    selectedLines,
    className,
    style,
    ...rest
  } = props;

  const patch = "patch" in props ? props.patch : undefined;
  const oldFile = "oldFile" in props ? props.oldFile : undefined;
  const newFile = "newFile" in props ? props.newFile : undefined;

  const mergedOptions = useMemo(
    () => buildDiffViewerOptions({ options, lightTheme, darkTheme, themeType }),
    [options, lightTheme, darkTheme, themeType],
  );

  const viewerStyle = useMemo(
    () => buildDiffViewerStyle({ style, fontSizePx }),
    [style, fontSizePx],
  );

  const containerClassName = diffViewerContainerClassName(className);
  return (
    <div {...rest} data-slot="diff-viewer" className={containerClassName} style={viewerStyle}>
      {patch !== undefined ? (
        <PatchDiff
          patch={patch}
          options={mergedOptions}
          selectedLines={selectedLines ?? undefined}
          disableWorkerPool={disableWorkerPool}
          className={diffClassName}
        />
      ) : oldFile && newFile ? (
        <MultiFileDiff
          oldFile={oldFile}
          newFile={newFile}
          options={mergedOptions}
          selectedLines={selectedLines ?? undefined}
          disableWorkerPool={disableWorkerPool}
          className={diffClassName}
        />
      ) : null}
    </div>
  );
}

export const DiffViewer = memo(DiffViewerComponent);
