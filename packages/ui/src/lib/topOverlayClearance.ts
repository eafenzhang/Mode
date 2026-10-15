/**
 * `DesktopTopOverlay` 左上操作入口（侧栏开关 / 前进后退 / 新建任务 / 更新按钮）的
 * 避让内边距：左侧栏收起时浮层不再被限制在侧栏宽度内，而是悬在主区顶栏左上（z-20），
 * 主区自带左内容的顶栏（chat 页 `WorkspaceHeader`、远程桌面会话面操作栏）必须让出这段
 * 宽度，否则设备名/标题会与入口按钮重叠（docs/specs/lan-remote-desktop.md UI 规则·会话面）。
 *
 * 数值与 `DesktopTopOverlay` 的平台内边距一一对应：mac 非全屏要把红绿灯占位
 * （`macWindowControlsLeftPaddingPx` ≈ 96px）一并让出；有更新按钮时入口再加宽一档。
 * 侧栏展开时浮层宽度 = `var(--workspace-sidebar-panel-width)`，不会伸进主区，返回 null。
 */
export function resolveTopOverlayClearancePaddingClass(options: {
  isSidebarVisible: boolean;
  isMacDesktop?: boolean;
  isMacFullscreen?: boolean;
  hasUpdateReady?: boolean;
}): string | null {
  if (options.isSidebarVisible) {
    return null;
  }

  if (options.isMacDesktop) {
    if (options.hasUpdateReady) {
      return options.isMacFullscreen ? "ps-48" : "ps-66";
    }
    return options.isMacFullscreen ? "ps-38" : "ps-58";
  }

  return options.hasUpdateReady ? "ps-44" : "ps-38";
}
