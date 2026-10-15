import {
  CONVERSATION_MIRROR_LIMITS,
  buildConversationMirrorImportedHistory,
  buildConversationMirrorRecord,
  buildMirrorImportSessionId,
  selectMirrorSourceTasks,
  shouldImportMirrorRecord,
  type ConversationMirrorSourceTask,
} from "@mode/shared/conversation-mirror";
import type { IModeSessionService, IModeTaskService, IServiceAccessor } from "@mode/services";

/**
 * 一轮远程工作区对话记录镜像同步（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 镜像产物是**真会话**，不是只读投影：把来源会话经 `session/create` + `importedHistory`
 * （`source: "remoteMirror"`）导入目标 scope，因此它可续聊、可置顶 / 归档 / 删除、进任务搜索。
 *
 * 两个方向（`viewServices` 是「当前视图那台机器」的服务；远端 tab 时即对端机器的代理）：
 * ① 对端本地 scope 记录 → 当前视图 scope（带 attach identity）；
 * ② 本机本地 scope 记录 → 对端本地 scope（不带 identity），让对端打开该工作区时也看到本机对话。
 *
 * 幂等与跳过判据都不依赖本地台账：目标 scope 自己的 task meta `updatedAt` 就是「已导入到哪一版」。
 */
export interface ConversationMirrorSyncParams {
  workspacePath: string;
  /** 当前视图 scope 的服务（远端 tab 为对端代理，本地 tab 为本机 host）。 */
  viewServices: IServiceAccessor;
  /** 当前视图 scope 的 workspaceIdentity：远端 attach 的身份；本地 tab 无。 */
  viewWorkspaceIdentity?: string;
  /** 本机 host 服务：读本机本地 scope 记录用（②方向的数据源）。 */
  localServices: IServiceAccessor;
  /** 本地工作区没有对端，不跑②方向。 */
  isRemoteTarget: boolean;
  onWarning?: (message: string, error?: unknown) => void;
}

export interface ConversationMirrorSyncResult {
  importedIntoView: number;
  importedIntoPeerLocal: number;
  /**
   * unsupported = 对端 CLI 不认识 `remoteMirror` 导入源（版本过低）。
   * 判定是启发式的：首个导入失败即中止本轮，避免对 N 条记录各撞一次同样的错误。
   *
   * degraded = 本轮有记录可读，但**一条都没读成功**（0 导入 + 有读取失败）。这跟「本来就没东西可同步」
   * 必须区分开：后者是 ready。典型场景是对端 CLI 没有冷读能力，`session/messages` 对每个冷会话都抛
   * `Session is not active`，于是整轮静默导入 0 条——只报 ready 会让这种全面失败永远看不见。
   */
  status: "ready" | "degraded" | "unsupported";
  /** unsupported / degraded 时的原始错误文案，供 UI / 日志诊断。 */
  error: string;
}

interface ImportScopeOutcome {
  imported: number;
  /** 来源消息读取失败条数；用来把「全面读失败」和「本来就没东西同步」区分开。 */
  readFailures: number;
  /** 最后一次读取失败的原文，供 degraded 状态提示。 */
  lastReadError: string;
  unsupportedError: string;
}

type SourceMessages = Awaited<ReturnType<IModeSessionService["readSessionMessages"]>>;

const MAX_MESSAGES_PER_READ = CONVERSATION_MIRROR_LIMITS.maxMessagesPerSession;

export async function runConversationMirrorSync(
  params: ConversationMirrorSyncParams,
): Promise<ConversationMirrorSyncResult> {
  const warn = params.onWarning ?? (() => undefined);
  if (!params.isRemoteTarget) {
    // 本地工作区没有对端：来源与目标是同一个 scope，导入只会把自己的会话复制成镜像副本。
    return {
      importedIntoView: 0,
      importedIntoPeerLocal: 0,
      status: "ready",
      error: "",
    };
  }

  // ① 对端本地 scope → 当前视图 scope。
  const peerLocal = await readScopeTasks(params.viewServices.modeTaskService, {
    workspacePath: params.workspacePath,
  });
  const viewTargets = await readScopeTasks(params.viewServices.modeTaskService, {
    workspacePath: params.workspacePath,
    ...(params.viewWorkspaceIdentity ? { workspaceIdentity: params.viewWorkspaceIdentity } : {}),
  });
  const viewOutcome = await importScopeRecords({
    sourceTasks: peerLocal.items,
    targetUpdatedAtById: indexByTaskId(viewTargets.items),
    sessionService: params.viewServices.modeSessionService,
    readSourceMessages: (taskId) =>
      params.viewServices.modeSessionService.readSessionMessages({
        workspacePath: params.workspacePath,
        sessionId: taskId,
        limit: MAX_MESSAGES_PER_READ,
      }),
    importWorkspace: {
      workspacePath: params.workspacePath,
      ...(params.viewWorkspaceIdentity ? { workspaceIdentity: params.viewWorkspaceIdentity } : {}),
    },
    warn,
  });
  if (viewOutcome.unsupportedError) {
    // 对端 CLI 版本过低：②方向共用同一台机器的 CLI，本轮一并跳过。
    return {
      importedIntoView: viewOutcome.imported,
      importedIntoPeerLocal: 0,
      status: "unsupported",
      error: viewOutcome.unsupportedError,
    };
  }

  // ② 本机本地 scope → 对端本地 scope（落在对端机器自己的本地工作区）。
  let importedIntoPeerLocal = 0;
  let localReadFailures = 0;
  let lastReadError = viewOutcome.lastReadError;
  if (params.isRemoteTarget) {
    const localSource = await readScopeTasks(params.localServices.modeTaskService, {
      workspacePath: params.workspacePath,
    });
    const localOutcome = await importScopeRecords({
      sourceTasks: localSource.items,
      targetUpdatedAtById: indexByTaskId(peerLocal.items),
      sessionService: params.viewServices.modeSessionService,
      readSourceMessages: (taskId) =>
        params.localServices.modeSessionService.readSessionMessages({
          workspacePath: params.workspacePath,
          sessionId: taskId,
          limit: MAX_MESSAGES_PER_READ,
        }),
      importWorkspace: { workspacePath: params.workspacePath },
      warn,
    });
    importedIntoPeerLocal = localOutcome.imported;
    localReadFailures = localOutcome.readFailures;
    if (localOutcome.lastReadError) {
      lastReadError = localOutcome.lastReadError;
    }
    if (localOutcome.unsupportedError) {
      return {
        importedIntoView: viewOutcome.imported,
        importedIntoPeerLocal,
        status: "unsupported",
        error: localOutcome.unsupportedError,
      };
    }
  }

  // 一条都没导入、却有读取失败：整轮是「读不出来」而不是「没东西可同步」。
  // 继续报 ready 会让这种全面失败在 UI 上永远不可见（组件在 ready 下渲染 null）。
  if (viewOutcome.imported + importedIntoPeerLocal === 0 && viewOutcome.readFailures + localReadFailures > 0) {
    return {
      importedIntoView: viewOutcome.imported,
      importedIntoPeerLocal,
      status: "degraded",
      error: lastReadError,
    };
  }

  return {
    importedIntoView: viewOutcome.imported,
    importedIntoPeerLocal,
    status: "ready",
    error: "",
  };
}

function indexByTaskId(items: readonly ConversationMirrorSourceTask[]): Map<string, number> {
  const map = new Map<string, number>();
  for (const item of items) {
    map.set(item.taskId, item.updatedAt);
  }
  return map;
}

async function readScopeTasks(
  modeTaskService: Pick<IModeTaskService, "listTaskList">,
  scope: { workspacePath: string; workspaceIdentity?: string },
): Promise<{ items: ConversationMirrorSourceTask[] }> {
  // 不带 identity 即「该机器在这个路径下的本地 scope 行」；任务列表查询是被动观察者，
  // 不会因此拉起任何 workspace runtime。
  const list = await modeTaskService.listTaskList({
    kind: "timeline",
    workspaceScopes: [scope],
    sortBy: "updated",
    limit: CONVERSATION_MIRROR_LIMITS.maxRecords * 2,
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

async function importScopeRecords(params: {
  sourceTasks: readonly ConversationMirrorSourceTask[];
  targetUpdatedAtById: Map<string, number>;
  sessionService: Pick<IModeSessionService, "createSession">;
  readSourceMessages: (taskId: string) => Promise<SourceMessages>;
  importWorkspace: { workspacePath: string; workspaceIdentity?: string };
  warn: (message: string, error?: unknown) => void;
}): Promise<ImportScopeOutcome> {
  let imported = 0;
  let readFailures = 0;
  let lastReadError = "";

  for (const task of selectMirrorSourceTasks(params.sourceTasks)) {
    const mirrorSessionId = buildMirrorImportSessionId(task.taskId);
    let messages: SourceMessages;
    try {
      messages = await params.readSourceMessages(task.taskId);
    } catch (error) {
      readFailures += 1;
      lastReadError = formatError(error);
      params.warn(`读取会话 ${task.taskId} 失败，已跳过`, error);
      continue;
    }

    const record = buildConversationMirrorRecord({ task, messages });
    if (!record) {
      // 没有可读文本（纯工具调用 / 全隐藏消息）：不导入空会话。
      continue;
    }
    if (
      !shouldImportMirrorRecord({
        record,
        targetUpdatedAt: params.targetUpdatedAtById.get(mirrorSessionId),
      })
    ) {
      continue;
    }

    try {
      await params.sessionService.createSession({
        sessionId: mirrorSessionId,
        ...params.importWorkspace,
        persistence: "immediate",
        importedHistory: buildConversationMirrorImportedHistory(record),
      });
      imported += 1;
    } catch (error) {
      // 首个失败即视为对端不支持该导入源（旧 CLI 的 strict schema 会拒绝 remoteMirror）：
      // 同一台机器上其余记录必然同样失败，继续尝试只会把同一个错误放大 N 次。
      return { imported, readFailures, lastReadError, unsupportedError: formatError(error) };
    }
  }

  return { imported, readFailures, lastReadError, unsupportedError: "" };
}

function formatError(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }
  return String(error);
}
