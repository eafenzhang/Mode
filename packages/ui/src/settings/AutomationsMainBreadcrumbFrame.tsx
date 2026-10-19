import { useState, type ReactNode } from "react";
import { DesktopWindowControls } from "@/DesktopWindowControls.js";
import { WorkspaceHelpMenuButton } from "@/WorkspaceHelpMenuButton.js";
import {
  SettingsBreadcrumbProvider,
  SettingsHeaderBreadcrumb,
  type SettingsBreadcrumbItem,
} from "@/settings/SettingsHeaderBreadcrumb.js";

/**
 * 工作区 Automations 不经过 SettingsPage，编辑页的面包屑上报需要 Provider 接收，
 * 所以桌面顶栏只剩空拖拽区；这里让工作区入口复用设置页的同一套面包屑合同。
 *
 * 全屏主视图（自动化/插件市场/远程控制）不渲染 WorkspaceHeader，窗控不会随 header 出现；
 * 这里在同一块拖拽区右侧补上帮助与自绘窗控，否则无边框窗口在这些页面没有最小化/关闭入口。
 */
export function AutomationsMainBreadcrumbFrame({
  ariaLabel,
  children,
  isDesktop,
  sectionLabel,
  showWindowControls = false,
}: {
  ariaLabel: string;
  children: ReactNode;
  isDesktop: boolean;
  sectionLabel: string;
  showWindowControls?: boolean;
}) {
  const [items, setItems] = useState<readonly SettingsBreadcrumbItem[]>([]);

  return (
    <SettingsBreadcrumbProvider onItemsChange={setItems} sectionLabel={sectionLabel}>
      <div className="flex min-h-0 flex-1 flex-col">
        {isDesktop ? (
          <div
            className="relative h-12 shrink-0 [app-region:drag]"
            data-testid="automations-main-drag-region"
          >
            <SettingsHeaderBreadcrumb ariaLabel={ariaLabel} items={items} />
            {showWindowControls ? (
              <div className="pointer-events-auto absolute end-2 top-0 flex h-12 items-center gap-0.5 [app-region:no-drag]">
                <WorkspaceHelpMenuButton isDesktop={Boolean(isDesktop)} />
                <DesktopWindowControls />
              </div>
            ) : null}
          </div>
        ) : null}
        {children}
      </div>
    </SettingsBreadcrumbProvider>
  );
}
