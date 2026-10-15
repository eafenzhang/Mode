import {
  CONVERSATION_MIRROR_LIMITS,
  buildConversationMirrorImportedHistory,
  buildConversationMirrorRecord,
  buildMirrorImportSessionId,
  mergeMirrorSourceTasks,
  selectMirrorSourceTasks,
  selectOwnMirrorSourceTasks,
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
 * ① 对端本地 scope 记录 → 当前视图 scope（带 attach identity），让本机看到对端的对话；
 * ② **本机在该工作区的记录** → 对端本地 scope（不带 identity），让对端打开该工作区时也看到本机的对话。
 *
 * ②方向的来源是「本机在这个工作区路径下的记录」，包含两个 scope：本机本地 scope（无 identity）
 * 与本机以对端身份打开该工作区时产生的会话（带 identity）。后者才是「本机在远端工作区发起的对话」，
 * 只取前者会在双机路径不同时恒为空——那会让本机发起的远端对话永远到不了对端。
 *
 * 同步窗口 = 调用方传入的 `windowSize`（侧栏分页窗口）：默认只处理最近 5 条，用户展开更多后
 * 调用方按新窗口重新触发一轮，因此导入量随可见列表增长，而不是一次性灌满。
 *
 * 幂等与跳过判据都不依赖本地台账：目标 scope 自己的 task meta `updatedAt` 就是「已导入到哪一版」。
 */
export interface ConversationMirrorSyncParams {
  workspacePath: string;
  /** 当前视图 scope 的服务（远端 tab 为对端代理，本地 tab 为本机 host）。 */
  viewServices: IServiceAccessor;
  /** 当前视图 scope 的 workspaceIdentity：远端 attach 的身份；本地 tab 无。 */
  viewWorkspaceIdentity?: string;
  /** 本机 host 服务：读本机本地 scope 记录用（②方向的数据源之一）。 */
  localServices: IServiceAccessor;
  /** 本地工作区没有对端，不跑②方向。 */
  isRemoteTarget: boolean;
  /** 本轮处理的来源会话条数（按 updatedAt 取最新前 N 条）。缺省取硬上限。 */
  windowSize?: number;
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

/** 一份来源记录 + 它所在的机器：消息必须回它自己那台机器读。 */
interface MirrorSourceScope {
  tasks: readonly ConversationMirrorSourceTask[];
  readMessages: (taskId: string) => Promise<SourceMessages>;
}

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

  const windowSize = resolveWindowSize(params.windowSize);
  const readLimit = resolveSourceReadLimit(windowSize);
  const readMessagesFrom = (
    services: IServiceAccessor,
    taskId: string,
  ): Promise<SourceMessages> =>
    services.modeSessionService.readSessionMessages({
      workspacePath: params.workspacePath,
      sessionId: taskId,
      limit: MAX_MESSAGES_PER_READ,
    });

  // ① 对端本地 scope → 当前视图 scope。
  const peerLocal = await readScopeTasks(params.viewServices.modeTaskService, {
    workspacePath: params.workspacePath,
    limit: readLimit,
  });
  const viewTargets = await readScopeTasks(params.viewServices.modeTaskService, {
    workspacePath: params.workspacePath,
    ...(params.viewWorkspaceIdentity ? { workspaceIdentity: params.viewWorkspaceIdentity } : {}),
    limit: readLimit,
  });
  const viewOutcome = await importScopeRecords({
    sources: [
      {
        tasks: peerLocal.items,
        readMessages: (taskId) => readMessagesFrom(params.viewServices, taskId),
      },
    ],
    selector: (tasks) => selectMirrorSourceTasks(tasks, windowSize),
    targetUpdatedAtById: indexByTaskId(viewTargets.items),
    sessionService: params.viewServices.modeSessionService,
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

  // ② 本机在该工作区的记录 → 对端本地 scope（落在对端机器自己的本地工作区）。
  let importedIntoPeerLocal = 0;
  let localReadFailures = 0;
  let lastReadError = viewOutcome.lastReadError;
  {
    const localScope = await readScopeTasks(params.localServices.modeTaskService, {
      workspacePath: params.workspacePath,
      limit: readLimit,
    });
    const localOutcome = await importScopeRecords({
      sources: [
        // 本机以对端身份打开该工作区时发起的对话（落在对端机器上）。
        {
          tasks: viewTargets.items,
          readMessages: (taskId) => readMessagesFrom(params.viewServices, taskId),
        },
        // 本机同路径本地工作区的对话（落在本机上）。
        {
          tasks: localScope.items,
          readMessages: (taskId) => readMessagesFrom(params.localServices, taskId),
        },
      ],
      selector: (tasks) => selectOwnMirrorSourceTasks(tasks, windowSize),
      targetUpdatedAtById: indexByTaskId(peerLocal.items),
      sessionService: params.viewServices.modeSessionService,
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

/** 窗口缺省取硬上限；非正数按 1 处理，避免把「窗口 0」误表达成「不同步」。 */
function resolveWindowSize(windowSize: number | undefined): number {
  if (typeof windowSize !== "number" || !Number.isFinite(windowSize)) {
    return CONVERSATION_MIRROR_LIMITS.maxRecords;
  }
  return Math.max(1, Math.min(Math.floor(windowSize), CONVERSATION_MIRROR_LIMITS.maxRecords));
}

/**
 * 任务列表读取上限：来源列表里混着镜像副本（会被跳过），窗口之上要多读一点给它们留位置。
 * 仍然有界——远端 RPC 的请求参数不能随窗口无限增长。
 */
function resolveSourceReadLimit(windowSize: number): number {
  return Math.min(
    CONVERSATION_MIRROR_LIMITS.maxRecords * 2,
    CONVERSATION_MIRROR_LIMITS.maxRecords + windowSize,
  );
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
  scope: { workspacePath: string; workspaceIdentity?: string; limit: number },
): Promise<{ items: ConversationMirrorSourceTask[] }> {
  // 不带 identity 即「该机器在这个路径下的本地 scope 行」；任务列表查询是被动观察者，
  // 不会因此拉起任何 workspace runtime。
  const list = await modeTaskService.listTaskList({
    kind: "timeline",
    workspaceScopes: [{ workspacePath: scope.workspacePath, ...(scope.workspaceIdentity ? { workspaceIdentity: scope.workspaceIdentity } : {}) }],
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

async function importScopeRecords(params: {
  sources: readonly MirrorSourceScope[];
  selector: (tasks: readonly ConversationMirrorSourceTask[]) => ConversationMirrorSourceTask[];
  targetUpdatedAtById: Map<string, number>;
  sessionService: Pick<IModeSessionService, "createSession">;
  importWorkspace: { workspacePath: string; workspaceIdentity?: string };
  warn: (message: string, error?: unknown) => void;
}): Promise<ImportScopeOutcome> {
  let imported = 0;
  let readFailures = 0;
  let lastReadError = "";

  const selected = params.selector(
    mergeMirrorSourceTasks(params.sources.map((source) => source.tasks)),
  );
  const readMessagesByTaskId = new Map<string, MirrorSourceScope["readMessages"]>();
  for (const source of params.sources) {
    for (const task of source.tasks) {
      readMessagesByTaskId.set(task.taskId, source.readMessages);
    }
  }

  for (const task of selected) {
    const readMessages = readMessagesByTaskId.get(task.taskId);
    const mirrorSessionId = buildMirrorImportSessionId(task.taskId);
    let messages: SourceMessages;
    try {
      if (!readMessages) {
        throw new Error(`镜像来源 ${task.taskId} 缺少读取通道`);
      }
      messages = await readMessages(task.taskId);
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
