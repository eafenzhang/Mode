import {
  CONVERSATION_MIRROR_LIMITS,
  buildConversationMirrorImportedHistory,
  buildConversationMirrorRecord,
  buildMirrorImportSessionId,
  isMirrorEligibleTask,
  isMirrorSessionId,
  mergeMirrorSourceTasks,
  resolveConversationMirrorWindowSize,
  selectMirrorSourceTasks,
  selectOwnMirrorSourceTasks,
  shouldImportMirrorRecord,
  shouldReadMirrorSource,
  type ConversationMirrorSourceTask,
} from "@mode/shared/conversation-mirror";
import type { IModeSessionService, IModeTaskService, IServiceAccessor } from "@mode/services";
import type { ConversationMirrorReadBackoff } from "@/lib/conversationMirrorReadBackoff.js";

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
 * 同步窗口 = 调用方传入的 `windowSize`（侧栏分页窗口）经 `resolveConversationMirrorWindowSize`
 * 归一：一轮至少处理最近 10 条、至多 20 条；用户展开更多后调用方按新窗口重新触发一轮，
 * 因此导入量随可见列表增长，而不是一次性灌满。
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
  /**
   * 本轮处理的来源会话条数基准（侧栏可见条数；按 updatedAt 取最新前 N 条）。
   * 实际窗口经 `resolveConversationMirrorWindowSize` 归一：至少 10 条、至多 20 条。
   */
  windowSize?: number;
  /**
   * 读取失败的退避表（可选）。传入后：失败过的来源在冷静期内不再重读，但本轮仍会按失败计入状态。
   * 不传即旧行为（每轮都重试），供只关心编排的调用方使用。
   */
  readBackoff?: ConversationMirrorReadBackoff;
  onWarning?: (message: string, error?: unknown) => void;
}

export interface ConversationMirrorSyncResult {
  importedIntoView: number;
  importedIntoPeerLocal: number;
  /**
   * unsupported = 对端 CLI 不认识 `remoteMirror` 导入源（版本过低）。
   * 只有「载荷被对端 schema 拒收」这一类才这么判（`-32602` / `-32601` / 错误文案点名导入字段），
   * 且首个失败即中止本轮，避免对 N 条记录各撞一次同样的错误。超时、网络中断等暂时性导入失败不算。
   *
   * degraded = 本轮有来源没同步上，却一条都没导入成功（0 导入 + 有读取失败 / 退避跳过 / 导入失败）。
   * 这跟「本来就没东西可同步」必须区分开：后者是 ready。典型场景是对端 CLI 没有冷读能力，
   * `session/messages` 对每个冷会话都抛 `Session is not active`，于是整轮静默导入 0 条——
   * 只报 ready 会让这种全面失败永远看不见。
   */
  status: "ready" | "degraded" | "unsupported";
  /** unsupported / degraded 时的原始错误文案，供 UI / 日志诊断。 */
  error: string;
}

interface ImportScopeOutcome {
  imported: number;
  /** 来源消息读取失败条数；用来把「全面读失败」和「本来就没东西同步」区分开。 */
  readFailures: number;
  /**
   * 处在退避期、本轮**没有重读**的失败来源条数。它不计入 `readFailures`（那会变成「每轮都撞同一个错」），
   * 但必须参与状态判定：否则持续失败的会话会被退避洗成 ready。
   */
  suppressedFailures: number;
  /**
   * 导入（`session/create`）失败条数。只有「对端不认这个导入源」才中止整轮；其余（超时、网络、
   * 对端繁忙）都是可重试的暂时失败，逐条跳过并计入状态，不能把一次抖动放大成整轮方向判死。
   */
  importFailures: number;
  /** 最后一次失败的原文（读取或导入），供 degraded 状态提示。 */
  lastError: string;
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

  // 侧栏可见条数只是基准：窗口口径统一在 shared（至少 10 条、至多 20 条）。
  const windowSize = resolveConversationMirrorWindowSize(params.windowSize);
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
  const peerLocal = await readScopeTasksForWindow(
    params.viewServices.modeTaskService,
    { workspacePath: params.workspacePath },
    keepAsPeerLocalSource,
    windowSize,
  );
  const viewTargets = await readScopeTasksForWindow(
    params.viewServices.modeTaskService,
    {
      workspacePath: params.workspacePath,
      ...(params.viewWorkspaceIdentity ? { workspaceIdentity: params.viewWorkspaceIdentity } : {}),
    },
    keepAsOwnSource,
    windowSize,
  );
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
    ...(params.readBackoff ? { readBackoff: params.readBackoff } : {}),
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
  let localSuppressedFailures = 0;
  let localImportFailures = 0;
  let lastError = viewOutcome.lastError;
  {
    const localScope = await readScopeTasksForWindow(
      params.localServices.modeTaskService,
      { workspacePath: params.workspacePath },
      keepAsOwnSource,
      windowSize,
    );
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
      ...(params.readBackoff ? { readBackoff: params.readBackoff } : {}),
      warn,
    });
    importedIntoPeerLocal = localOutcome.imported;
    localReadFailures = localOutcome.readFailures;
    localSuppressedFailures = localOutcome.suppressedFailures;
    localImportFailures = localOutcome.importFailures;
    if (localOutcome.lastError) {
      lastError = localOutcome.lastError;
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

  // 一条都没导入、却有「没同步上」的来源：整轮是「同步失败」而不是「没东西可同步」。
  // 继续报 ready 会让这种全面失败在 UI 上永远不可见（组件在 ready 下渲染 null）。
  // 退避跳过同样计入：持续失败的来源在冷静期内不再重读，但它依然没同步上，不能因此变成 ready。
  // 导入失败（可重试的暂时失败）也要计入，否则「读得到但写不进去」会被洗成 ready。
  const unsyncedSources =
    viewOutcome.readFailures +
    localReadFailures +
    viewOutcome.suppressedFailures +
    localSuppressedFailures +
    viewOutcome.importFailures +
    localImportFailures;
  if (viewOutcome.imported + importedIntoPeerLocal === 0 && unsyncedSources > 0) {
    return {
      importedIntoView: viewOutcome.imported,
      importedIntoPeerLocal,
      status: "degraded",
      error: lastError,
    };
  }

  return {
    importedIntoView: viewOutcome.imported,
    importedIntoPeerLocal,
    status: "ready",
    error: "",
  };
}

/**
 * 任务列表读取的**起始**上限：来源列表里混着镜像副本（会被跳过），窗口之上多读一点给它们留位置。
 * 一个混着副本的列表里，「多读一点」并不保证够——不够时由 `readScopeTasksForWindow` 逐级补页。
 */
function resolveSourceReadLimit(windowSize: number): number {
  return Math.min(
    CONVERSATION_MIRROR_LIMITS.maxRecords * 2,
    CONVERSATION_MIRROR_LIMITS.maxRecords + windowSize,
  );
}

/**
 * 单次来源列表读取的深度上限。补页不能无界，否则「来源 scope 里全是镜像副本」会退化成把整张
 * 任务表拉回来。
 */
const MIRROR_SOURCE_DEEP_READ_LIMIT = 200;

/**
 * ①方向来源的可用性判据：只认对端本地 scope（无 identity）且非镜像副本。
 * 与 `selectMirrorSourceTasks` 取的是同一个谓词，这里只是在**取列表时**就要用它决定是否补页。
 */
function keepAsPeerLocalSource(task: ConversationMirrorSourceTask): boolean {
  return isMirrorEligibleTask(task) && !isMirrorSessionId(task.taskId);
}

/**
 * ②方向来源的可用性判据：保留 identity 行（本机在远端工作区发起的对话），只跳过镜像副本。
 * 与 `selectOwnMirrorSourceTasks` 同谓词；两个方向不能共用（不变量 2）。
 */
function keepAsOwnSource(task: ConversationMirrorSourceTask): boolean {
  return !isMirrorSessionId(task.taskId);
}

/**
 * 取一页来源列表，并在页内可用来源不足 `windowSize` 时逐级补页。
 *
 * 列表按 `updatedAt` 取最新 N 条，而镜像副本会长期留在来源 scope 里（它们的 `updatedAt` 是来源的
 * 真实时间，排位靠前）。固定取一页的话，对端本地 scope 的头部被副本占满时，这一页里真实来源就不足
 * `windowSize` 条，**更旧的来源会永远同步不过来**。所以这里按「可用条数」而不是「总条数」判停。
 *
 * 停止条件：可用来源已够 / 这一页本来就不满（列表到底了）/ 已到深度上限。
 */
async function readScopeTasksForWindow(
  modeTaskService: Pick<IModeTaskService, "listTaskList">,
  scope: { workspacePath: string; workspaceIdentity?: string },
  keep: (task: ConversationMirrorSourceTask) => boolean,
  windowSize: number,
): Promise<{ items: ConversationMirrorSourceTask[] }> {
  let limit = resolveSourceReadLimit(windowSize);
  for (;;) {
    const page = await readScopeTasks(modeTaskService, { ...scope, limit });
    const usable = page.items.reduce((count, task) => (keep(task) ? count + 1 : count), 0);
    if (usable >= windowSize || page.items.length < limit || limit >= MIRROR_SOURCE_DEEP_READ_LIMIT) {
      return page;
    }
    limit = Math.min(limit * 2, MIRROR_SOURCE_DEEP_READ_LIMIT);
  }
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
  readBackoff?: ConversationMirrorReadBackoff;
  warn: (message: string, error?: unknown) => void;
}): Promise<ImportScopeOutcome> {
  let imported = 0;
  let readFailures = 0;
  let suppressedFailures = 0;
  let importFailures = 0;
  let lastError = "";

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
    const targetUpdatedAt = params.targetUpdatedAtById.get(mirrorSessionId);
    // 水位在读取之前就已知：目标端已是最新版时连消息都不读。
    // 读消息是整轮最贵的一步，「先读后判」会让每轮空转都付出满负荷代价（spec 第四轮修复 成因 A）。
    if (!shouldReadMirrorSource({ task, targetUpdatedAt })) {
      continue;
    }
    // 稳定读不出来的来源（例如 wire 校验拒收的 part 形态）在冷静期内不再重试，
    // 但仍然计入状态：跳过不是为了把它当成「已同步」。
    const suppressedError = params.readBackoff?.shouldSkip(task.taskId);
    if (suppressedError !== undefined) {
      suppressedFailures += 1;
      if (!lastError) {
        lastError = suppressedError;
      }
      continue;
    }

    let messages: SourceMessages;
    try {
      if (!readMessages) {
        throw new Error(`镜像来源 ${task.taskId} 缺少读取通道`);
      }
      messages = await readMessages(task.taskId);
    } catch (error) {
      readFailures += 1;
      const detail = formatError(error);
      lastError = detail;
      params.readBackoff?.recordFailure(task.taskId, detail);
      params.warn(`读取会话 ${task.taskId} 失败，已跳过：${boundLogText(detail)}`);
      continue;
    }
    params.readBackoff?.recordSuccess(task.taskId);

    const record = buildConversationMirrorRecord({ task, messages });
    if (!record) {
      // 没有可读文本（纯工具调用 / 全隐藏消息）：不导入空会话。
      continue;
    }
    if (
      !shouldImportMirrorRecord({
        record,
        targetUpdatedAt,
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
      const detail = formatError(error);
      if (isUnsupportedImportSourceError(error)) {
        // 对端不认这个导入源（旧 CLI 的 strict schema 拒绝 remoteMirror）：同一台机器上其余记录
        // 必然同样失败，首个失败即中止该方向，不再把同一个错误放大 N 次。
        return {
          imported,
          readFailures,
          suppressedFailures,
          importFailures,
          lastError,
          unsupportedError: detail,
        };
      }
      // 其余失败（超时 / 网络 / 对端繁忙 / 磁盘错误）都是可重试的暂时失败：只跳过这一条并继续，
      // 否则一次抖动就会把整轮方向判死，还会在 UI 上给出「对端版本过低」这个错误理由。
      importFailures += 1;
      lastError = detail;
      params.warn(`导入会话 ${task.taskId} 失败，已跳过：${boundLogText(detail)}`);
    }
  }

  return { imported, readFailures, suppressedFailures, importFailures, lastError, unsupportedError: "" };
}

/**
 * 对端是否**因为不认这个导入源**而拒绝。只有这一类才是「对端版本过低」：
 *
 * - `-32602`：旧 CLI 的 strict schema 把 `importedHistory` 判为 invalid params（校验路径会出现在
 *   message 里，见 `summarizeParamsError`）；
 * - `-32601`：更旧的 CLI 连 `session/create` 都没有。
 *
 * 其余错误（超时、连接中断、对端 busy）都不该按版本问题处理——把它们和「版本过低」混为一谈，
 * 会让一次抖动把整轮方向判死并给出错误的用户提示。
 */
function isUnsupportedImportSourceError(error: unknown): boolean {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? (error as { code?: unknown }).code
      : undefined;
  if (code === -32602 || code === -32601) {
    return true;
  }
  const text = formatError(error).toLowerCase();
  return text.includes("remotemirror") || text.includes("importedhistory");
}

/**
 * 单条日志的错误文案上限。ZodError 的 message 会把每个校验 issue 全部序列化出来
 * （实测有单条 2.9 MB 的日志行），进日志前必须截断。
 */
const MAX_LOG_ERROR_CHARS = 400;

function boundLogText(text: string): string {
  return text.length <= MAX_LOG_ERROR_CHARS
    ? text
    : `${text.slice(0, MAX_LOG_ERROR_CHARS)}…（共 ${text.length} 字符，已截断）`;
}

function formatError(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }
  return String(error);
}
