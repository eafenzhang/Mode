import { MODE_VERSION } from "@mode/shared";
import { cn } from "@/components/lib/utils.js";

/**
 * 左下角构建版本号。`MODE_VERSION` 是各 bundler 注入的只读常量，Web/桌面同源；
 * 这里不读设置或远端 workspace，避免同一个构建在不同 workspace 显示不同版本。
 */
export function SidebarAppVersion({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        "shrink-0 select-none text-ui-sm font-normal text-foreground-subtlest",
        className,
      )}
      data-testid="sidebar-app-version"
      title={`v${MODE_VERSION}`}
    >
      v{MODE_VERSION}
    </span>
  );
}
