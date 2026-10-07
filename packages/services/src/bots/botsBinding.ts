import type { BotWorkspaceRef } from "@zcode/shared";

/**
 * 工作区 → bot[] 绑定表的纯函数操作。绑定持久化在 bot-bindings.v3.json：
 * key 为 workspaceKey（workspaceIdentity 优先于 workspacePath），value 为该工作区的 botId 列表。
 * 语义：一个工作区可以绑定多个 bot；一个 bot 也可以被多个工作区绑定——它的上下文
 * 跟随 UI 焦点在绑定的工作区之间切换。绑定优先于 bot 的自由 /workspace 切换。
 */

export type BotWorkspaceBindings = Record<string, string[]>;

/** 某个工作区绑定的 bot 列表（未绑定返回空数组）。 */
export function getWorkspaceBoundBots(
  bindings: BotWorkspaceBindings | undefined,
  workspaceKey: string,
): string[] {
  return bindings?.[workspaceKey] ?? [];
}

/** 追加绑定（幂等）：工作区已有该 bot 时返回原对象引用，避免无意义落盘。 */
export function setWorkspaceBinding(
  bindings: BotWorkspaceBindings | undefined,
  workspaceKey: string,
  botId: string,
): BotWorkspaceBindings {
  const current = getWorkspaceBoundBots(bindings, workspaceKey);
  if (current.includes(botId)) {
    return bindings ?? {};
  }
  return { ...bindings, [workspaceKey]: [...current, botId] };
}

/** 解绑单个 bot；该工作区没有其他 bot 时整条记录删除。 */
export function removeWorkspaceBinding(
  bindings: BotWorkspaceBindings | undefined,
  workspaceKey: string,
  botId: string,
): BotWorkspaceBindings {
  const current = getWorkspaceBoundBots(bindings, workspaceKey);
  if (!current.includes(botId)) {
    return bindings ?? {};
  }
  const next = { ...bindings };
  const remaining = current.filter((item) => item !== botId);
  if (remaining.length === 0) {
    delete next[workspaceKey];
  } else {
    next[workspaceKey] = remaining;
  }
  return next;
}

/** 一个 bot 只能绑定一个工作区：把它从除 keepWorkspaceKey 之外的所有工作区列表里摘掉。 */
export function removeBotFromOtherWorkspaces(
  bindings: BotWorkspaceBindings | undefined,
  keepWorkspaceKey: string,
  botId: string,
): BotWorkspaceBindings {
  let next = bindings ?? {};
  for (const workspaceKey of Object.keys(next)) {
    if (workspaceKey === keepWorkspaceKey) {
      continue;
    }
    next = removeWorkspaceBinding(next, workspaceKey, botId);
  }
  return next;
}

/**
 * 解析某个 bot 当前被绑定的工作区（对照已知工作区列表自愈）。
 * 绑定 key 只存 workspaceKey；工作区从列表里消失（关闭/归档）时视为未绑定，
 * 避免把 bot 钉在一个已经不存在的上下文上。
 */
export function resolveBoundWorkspaceRefs(
  bindings: BotWorkspaceBindings | undefined,
  botId: string,
  workspaceRefs: readonly BotWorkspaceRef[],
): BotWorkspaceRef[] {
  if (!bindings) {
    return [];
  }
  const byId = new Map(workspaceRefs.map((ref) => [ref.id, ref]));
  const resolved: BotWorkspaceRef[] = [];
  for (const [workspaceKey, boundBotIds] of Object.entries(bindings)) {
    if (!boundBotIds.includes(botId)) {
      continue;
    }
    const ref = byId.get(workspaceKey);
    if (ref) {
      resolved.push(ref);
    }
  }
  return resolved;
}
