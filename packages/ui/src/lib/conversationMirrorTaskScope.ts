/**
 * 镜像同步用到的任务列表读取：来源补页的列表读取 + 目标水位（含置顶 / 归档分区兜底）。
 *
 * 从 `conversationMirrorSync.ts` 拆出来，避免编排文件超过行数门禁；语义见
 * docs/specs/remote-workspace-conversation-mirror.md 与各函数注释。
 */
import type { IModeTaskService, ModeTaskListKind } from "@mode/services";
import type { ConversationMirrorSourceTask } from "@mode/shared/conversation-mirror";

/**
 * 单次来源列表读取的深度上限（来源补页与水位兜底共用）。补页不能无界，否则
 * 「来源 scope 里全是镜像副本」会退化成把整张任务表拉回来。
 */
export const MIRROR_SOURCE_DEEP_READ_LIMIT = 200;

/** 单条日志的错误文案上限，与 `conversationMirrorSync.ts` 的 `boundLogText` 同口径。 */
const MAX_LOG_ERROR_CHARS = 400;

export async function readScopeTasks(
  modeTaskService: Pick<IModeTaskService, "listTaskList">,
  scope: {
    workspacePath: string;
    workspaceIdentity?: string;
    limit: number;
    kind?: ModeTaskListKind;
  },
): Promise<{ items: ConversationMirrorSourceTask[] }> {
  // 不带 identity 即「该机器在这个路径下的本地 scope 行」；任务列表查询是被动观察者，
  // 不会因此拉起任何 workspace runtime。
  const list = await modeTaskService.listTaskList({
    kind: scope.kind ?? "timeline",
    workspaceScopes: [
      {
        workspacePath: scope.workspacePath,
        ...(scope.workspaceIdentity ? { workspaceIdentity: scope.workspaceIdentity } : {}),
      },
    ],
    sortBy: "updated",
    limit: scope.limit,
  });
  return {
    items: list.items.map((item) => ({
      taskId: item.taskId,
      title: item.title,
      createdAt: item.createdAt,
      updatedAt: item.updatedAt,
      ...(item.workspaceIdentity ? { workspaceIdentity: item.workspaceIdentity } : {}),
    })),
  };
}

export function indexByTaskId(
  items: readonly ConversationMirrorSourceTask[],
): Map<string, number> {
  const map = new Map<string, number>();
  for (const item of items) {
    map.set(item.taskId, item.updatedAt);
  }
  return map;
}

/**
 * 目标 scope 的水位查询：timeline 分区之外，必要时补齐置顶 / 归档分区。
 *
 * `listTaskList` 的 `kind:"timeline"` 只返回 `pinned=0 AND archived=0` 的行。
 * 镜像副本的 `updatedAt` 是来源的真实时间（第三轮回填），超过自动归档阈值（默认 7 天）时
 * 一导入就被归档；用户也可能把它置顶。水位是「已导入到哪一版」的记账，不是侧栏可见性——
 * 只看 timeline 会让副本每轮都命中「目标端没有该会话」，于是按 20s 下限无限重导
 * （每轮 `session/create` 都新建一套 runtime 与 MCP 子进程）。这里只在 timeline 命中不了时
 * 懒加载另外两个分区：常态零额外 RPC，命中归档 / 置顶副本时补齐后即恢复跳过。
 */
export function createTargetWatermarkLookup(params: {
  modeTaskService: Pick<IModeTaskService, "listTaskList">;
  scope: { workspacePath: string; workspaceIdentity?: string };
  base: Map<string, number>;
  warn: (message: string, error?: unknown) => void;
}): (taskId: string) => Promise<number | undefined> {
  let extra: Map<string, number> | null = null;
  let loading: Promise<void> | null = null;
  const load = async (): Promise<void> => {
    const limit = MIRROR_SOURCE_DEEP_READ_LIMIT;
    const [pinned, archived] = await Promise.all([
      readScopeTasks(params.modeTaskService, { ...params.scope, limit, kind: "pinned" }),
      readScopeTasks(params.modeTaskService, { ...params.scope, limit, kind: "archived" }),
    ]);
    extra = indexByTaskId([...pinned.items, ...archived.items]);
  };
  return async (taskId) => {
    const direct = params.base.get(taskId);
    if (direct !== undefined) {
      return direct;
    }
    if (!extra) {
      loading ??= load().catch((error: unknown) => {
        params.warn(`读取目标 scope 的置顶 / 归档分区失败：${boundLogError(error)}`);
      });
      await loading;
    }
    return extra?.get(taskId);
  };
}

function boundLogError(error: unknown): string {
  const text = error instanceof Error && error.message.trim() ? error.message : String(error);
  return text.length <= MAX_LOG_ERROR_CHARS
    ? text
    : `${text.slice(0, MAX_LOG_ERROR_CHARS)}…（共 ${text.length} 字符，已截断）`;
}
