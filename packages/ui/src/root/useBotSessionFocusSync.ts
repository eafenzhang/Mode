import { useEffect } from "react";
import type { IServiceAccessor } from "@mode/services";
import { logger } from "@/logger.js";
import { useModeSessionStore } from "@/store/modeSessionStore.js";
import { useTabStoreApi } from "@/store/TabStoreProvider.js";
import { isWorkspaceTab } from "@/store/tabStore.js";

/**
 * UI 会话/工作区焦点 → bot 的单向同步。
 *
 * 订阅 tab store（激活 tab / 工作区切换）与 session store（per-workspace activeTaskId），
 * 焦点签名变化（去重 + microtask 合并）后调用 botsService.notifyUiSessionFocus，
 * 让绑定到当前工作区的 bot 跟随 UI 的会话焦点。未绑定 bot 时 service 侧直接 no-op。
 *
 * 反方向（bot 侧 /task.set、bot 新建任务）经 active_task_changed 广播让 UI 跳转后，
 * 这里的通知会命中 service 层 focusBotOnWorkspace 的幂等短路，双向联动不会成环。
 */
export function useBotSessionFocusSync(
  services: IServiceAccessor,
  tabStoreApi: ReturnType<typeof useTabStoreApi>,
) {
  useEffect(() => {
    let lastSignature: string | null = null;
    let scheduled = false;

    const run = () => {
      const tabState = tabStoreApi.getState();
      const activeTab = tabState.tabs.find((tab) => tab.id === tabState.activeTabId);
      if (!activeTab || !isWorkspaceTab(activeTab)) {
        lastSignature = null;
        return;
      }
      const workspaceState = useModeSessionStore
        .getState()
        .getWorkspaceState(activeTab.workspacePath, activeTab.workspaceIdentity);
      const workspaceKey = activeTab.workspaceIdentity?.trim() || activeTab.workspacePath;
      const signature = `${workspaceKey}\u0000${workspaceState.activeTaskId ?? ""}`;
      if (signature === lastSignature) {
        return;
      }
      lastSignature = signature;
      void services.botsService
        .notifyUiSessionFocus({
          workspacePath: activeTab.workspacePath,
          ...(activeTab.workspaceIdentity ? { workspaceIdentity: activeTab.workspaceIdentity } : {}),
          taskId: workspaceState.activeTaskId,
        })
        .catch((error: unknown) => {
          logger.warn(
            `[BotSessionFocusSync] notifyUiSessionFocus failed: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        });
    };

    const schedule = () => {
      if (scheduled) {
        return;
      }
      scheduled = true;
      queueMicrotask(() => {
        scheduled = false;
        run();
      });
    };

    // 挂载时同步一次当前焦点；bot 上下文已是目标时 service 幂等短路。
    run();
    const unsubscribeTab = tabStoreApi.subscribe(schedule);
    const unsubscribeSession = useModeSessionStore.subscribe(schedule);
    return () => {
      unsubscribeTab();
      unsubscribeSession();
    };
  }, [services, tabStoreApi]);
}
