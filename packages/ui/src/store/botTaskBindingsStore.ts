import { create } from "zustand";
import type { BotProvider } from "@mode/shared";
import type { IBotsService } from "@mode/services";

/**
 * 任务 ↔ IM 机器人绑定表（桌面侧只读投影）。
 *
 * 数据源是 botsService.listBotTaskBindings()：任务行的绿点标识与会话右键菜单
 * 都从这里读。一个会话可以同时绑定多个机器人（同一工作区的多个 bot），
 * 因此每个任务对应一个列表。
 * 刷新时机：Root 挂载时拉一次 + 每次 bots:task 广播后防抖重拉 + 绑定动作后立即拉
 * （工作区级绑定会把该工作区的 bot 钉到当前会话，不等广播也要让绿点/菜单立刻反映）。
 */

export interface BotTaskBinding {
  botId: string;
  provider: BotProvider;
  /** 绑定到该会话的 IM 对话（同一个 bot 可以有多个对话各自绑不同会话）。 */
  conversationKey: string;
  conversationKind: "private" | "group";
  conversationId: string;
  conversationLabel?: string;
}

interface BotTaskBindingsState {
  bindings: Record<string, BotTaskBinding[]>;
  setBindings: (bindings: Record<string, BotTaskBinding[]>) => void;
}

export function buildBotTaskBindingKey(
  workspacePath: string,
  workspaceIdentity: string | undefined,
  taskId: string,
): string {
  return `${workspaceIdentity?.trim() || workspacePath}\u0000${taskId}`;
}

export const useBotTaskBindingsStore = create<BotTaskBindingsState>((set) => ({
  bindings: {},
  setBindings: (bindings) => set({ bindings }),
}));

const EMPTY_BINDINGS: BotTaskBinding[] = [];

/** 读取某任务绑定的机器人列表（无绑定返回空数组；同一任务多个 bot 时全部返回）。 */
export function useBotTaskBinding(
  workspacePath: string,
  workspaceIdentity: string | undefined,
  taskId: string,
): BotTaskBinding[] {
  return useBotTaskBindingsStore(
    (state) =>
      state.bindings[buildBotTaskBindingKey(workspacePath, workspaceIdentity, taskId)] ??
      EMPTY_BINDINGS,
  );
}

/** 拉取绑定表并按任务聚合（同一会话的多个 bot 合并成一个列表，不能只留最后一个）。 */
export async function refreshBotTaskBindings(
  botsService: Pick<IBotsService, "listBotTaskBindings">,
): Promise<void> {
  const bindings = await botsService.listBotTaskBindings();
  const next: Record<string, BotTaskBinding[]> = {};
  for (const binding of bindings) {
    const key = buildBotTaskBindingKey(
      binding.workspacePath,
      binding.workspaceIdentity,
      binding.taskId,
    );
    const list = next[key] ?? (next[key] = []);
    if (
      !list.some(
        (item) =>
          item.botId === binding.botId && item.conversationKey === binding.conversationKey,
      )
    ) {
      list.push({
        botId: binding.botId,
        provider: binding.provider,
        conversationKey: binding.conversationKey,
        conversationKind: binding.conversationKind,
        conversationId: binding.conversationId,
        ...(binding.conversationLabel ? { conversationLabel: binding.conversationLabel } : {}),
      });
    }
  }
  useBotTaskBindingsStore.getState().setBindings(next);
}
