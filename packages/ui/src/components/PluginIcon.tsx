import { useState, type ReactNode } from "react";
import { Blocks } from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { resolvePluginIconSource } from "@/lib/pluginIconSource.js";

/**
 * Plugin 原始图标：优先官方内置资源与目录条目自带的 https 图标；
 * 目录/插件都没有提供图标时用默认图标（Blocks），不再另外生成。
 */
export function PluginIcon({
  src,
  pluginId,
  className,
  iconClassName,
  fallbackIcon,
}: {
  src?: string;
  pluginId?: string;
  className?: string;
  iconClassName?: string;
  fallbackIcon?: ReactNode;
}) {
  const [imageFailed, setImageFailed] = useState(false);
  const resolvedSrc = resolvePluginIconSource(pluginId, src);
  const showImage = Boolean(resolvedSrc) && !imageFailed;
  return (
    <span
      aria-hidden="true"
      className={cn(
        "flex shrink-0 select-none items-center justify-center rounded-xl bg-surface",
        !showImage && "text-foreground-subtle",
        className,
      )}
    >
      {showImage ? (
        <img
          src={resolvedSrc}
          alt=""
          draggable={false}
          className="h-2/3 w-2/3 object-contain"
          onError={() => setImageFailed(true)}
        />
      ) : (
        (fallbackIcon ?? <Blocks className={cn("size-4", iconClassName)} />)
      )}
    </span>
  );
}
