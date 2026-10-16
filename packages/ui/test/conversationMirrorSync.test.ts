import assert from "node:assert/strict";
import test from "node:test";
import { buildMirrorImportSessionId } from "@mode/shared/conversation-mirror";
import { runConversationMirrorSync } from "../src/lib/conversationMirrorSync.js";
import { createConversationMirrorReadBackoff } from "../src/lib/conversationMirrorReadBackoff.js";

/**
 * 远程工作区对话记录镜像的编排回归（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 夹具按真实拓扑搭两台机器：远端机的「本地 scope」是①方向的来源、又是②方向的目标；
 * 远端机的「视图 scope」（带 identity）是①方向的目标、又是②方向的来源（本机连上远端工作区后
 * 在其中发起的对话就落在这里）；本机只提供②方向的另一个来源（同路径本地 scope）。
 * 两边共用一份服务桩会让来源与目标重合，掩盖方向接错的问题。
 */

const WORKSPACE_PATH = "D:/proj";
const VIEW_IDENTITY = "remote:lan:192.168.1.20:45880:D:/proj";

interface TaskRow {
  taskId: string;
  title?: string;
  updatedAt: number;
}

interface CreateCall {
  sessionId: string;
  workspacePath: string;
  workspaceIdentity?: string;
  persistence?: string;
  importedHistory?: { source?: string; originSessionId?: string };
}

interface MachineParams {
  /** 该机器上「不带 identity」的本地 scope 行。 */
  localScope?: TaskRow[];
  /** 该机器上「带 identity」（视图 scope）的行。 */
  viewScope?: TaskRow[];
  readMessages?: (taskId: string) => Promise<unknown>;
  createSession?: (params: CreateCall) => Promise<unknown>;
}

function textMessage(text: string): unknown {
  return {
    info: {
      messageId: `m-${text}`,
      sessionId: "s1",
      role: "user",
      time: { created: 1 },
      agent: "mode-agent",
    },
    parts: [{ partId: `p-${text}`, sessionId: "s1", messageId: `m-${text}`, type: "text", text }],
  };
}

function createMachine(params: MachineParams) {
  const createCalls: CreateCall[] = [];
  const readCalls: string[] = [];
  const listScopes: Array<{ workspacePath: string; workspaceIdentity?: string }> = [];
  /** 每次来源列表读取请求的 limit，用来观测「一页装不下可用来源时逐级补页」。 */
  const requestedLimits: number[] = [];

  const modeTaskService = {
    async listTaskList(query: {
      workspaceScopes: Array<{ workspacePath: string; workspaceIdentity?: string }>;
      limit?: number;
    }) {
      const scope = query.workspaceScopes[0] ?? { workspacePath: WORKSPACE_PATH };
      listScopes.push(scope);
      requestedLimits.push(query.limit ?? 0);
      const rows = scope.workspaceIdentity ? (params.viewScope ?? []) : (params.localScope ?? []);
      // 真实 listTaskList 按 updatedAt 取「最新 N 条」；夹具数组本身就是「新 → 旧」顺序，
      // 所以直接截前 N 条即可，让补页行为在测试里可观测。
      const page = typeof query.limit === "number" ? rows.slice(0, query.limit) : rows;
      return {
        items: page.map((row) => ({
          taskId: row.taskId,
          traceId: `trace-${row.taskId}`,
          title: row.title ?? row.taskId,
          workspacePath: scope.workspacePath,
          createdAt: 1,
          updatedAt: row.updatedAt,
          mode: "agent",
          status: "completed",
          hasUnread: false,
          pinned: false,
          archived: false,
        })),
        total: rows.length,
        hasMore: page.length < rows.length,
      };
    },
  };

  const modeSessionService = {
    async readSessionMessages(input: { sessionId: string }) {
      readCalls.push(input.sessionId);
      if (params.readMessages) {
        return await params.readMessages(input.sessionId);
      }
      return [textMessage(`内容-${input.sessionId}`)];
    },
    async createSession(createParams: CreateCall) {
      createCalls.push(createParams);
      if (params.createSession) {
        return await params.createSession(createParams);
      }
      return {};
    },
  };

  return {
    services: { modeTaskService, modeSessionService } as never,
    createCalls,
    readCalls,
    listScopes,
    requestedLimits,
  };
}

/** 视图方向（①）的导入：带 identity，落在对端机器上「以本机身份打开的那个 scope」。 */
function viewDirectionCalls(calls: readonly CreateCall[]): CreateCall[] {
  return calls.filter((call) => typeof call.workspaceIdentity === "string");
}

/** 反向（②）的导入：不带 identity，落在对端机器自己的本地 scope。 */
function peerLocalCalls(calls: readonly CreateCall[]): CreateCall[] {
  return calls.filter((call) => !("workspaceIdentity" in call));
}

test("两个方向各落一个 scope：对端记录进视图 scope（带 identity），本机记录进对端本地 scope（不带）", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", title: "对端会话", updatedAt: 500 }],
  });
  const local = createMachine({
    localScope: [{ taskId: "mine-1", title: "本机会话", updatedAt: 700 }],
  });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 1);
  assert.equal(result.importedIntoPeerLocal, 1);

  const [viewImport] = viewDirectionCalls(remote.createCalls);
  assert.ok(viewImport, "①方向应当导入一条对端会话");
  assert.equal(viewImport.sessionId, buildMirrorImportSessionId("peer-1"));
  assert.equal(viewImport.workspacePath, WORKSPACE_PATH);
  assert.equal(viewImport.workspaceIdentity, VIEW_IDENTITY);
  assert.equal(viewImport.persistence, "immediate");
  assert.equal(viewImport.importedHistory?.source, "remoteMirror");
  assert.equal(viewImport.importedHistory?.originSessionId, "peer-1");

  const [peerImport] = peerLocalCalls(remote.createCalls);
  assert.ok(peerImport, "②方向应当把本机会话导到对端机器上");
  assert.equal(peerImport.sessionId, buildMirrorImportSessionId("mine-1"));
  assert.equal(peerImport.workspacePath, WORKSPACE_PATH);
  assert.equal(peerImport.persistence, "immediate");
  assert.equal(peerImport.importedHistory?.originSessionId, "mine-1");

  // 来源必须来自各自的机器：①读对端、②读本机；写则都落在承载工作区的对端机器上。
  assert.deepEqual(remote.readCalls, ["peer-1"]);
  assert.deepEqual(local.readCalls, ["mine-1"]);
  assert.equal(local.createCalls.length, 0);
});

test("目标 scope 已有同版本镜像会话时不重复导入", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", title: "对端会话", updatedAt: 500 }],
    viewScope: [
      { taskId: buildMirrorImportSessionId("peer-1"), title: "上次导入的", updatedAt: 900 },
    ],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 0);
  assert.equal(remote.createCalls.length, 0);
});

test("镜像副本（mirror- 前缀）既不被当成新来源导入视图 scope，也不会被再搬一次", async () => {
  const remote = createMachine({
    localScope: [
      { taskId: "peer-1", title: "对端会话", updatedAt: 500 },
      {
        taskId: buildMirrorImportSessionId("mine-1"),
        title: "上一轮导到对端的那份副本",
        updatedAt: 900,
      },
    ],
  });
  const local = createMachine({ localScope: [{ taskId: "mine-1", updatedAt: 950 }] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 1);
  assert.equal(result.importedIntoPeerLocal, 1);

  // ①只导入对端真实会话：对端那份 mirror- 副本没有被当成新记录再导进视图 scope。
  assert.deepEqual(
    viewDirectionCalls(remote.createCalls).map((call) => call.sessionId),
    [buildMirrorImportSessionId("peer-1")],
  );
  // ②本机会话更新（950 > 900）时正常导入，且只导入一次。
  assert.deepEqual(
    peerLocalCalls(remote.createCalls).map((call) => call.sessionId),
    [buildMirrorImportSessionId("mine-1")],
  );
});

test("来源未更新（目标 meta 更旧才导）时不重导，用户续聊过的镜像会话不被覆盖", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", updatedAt: 400 }],
    viewScope: [
      // 目标比来源更新：镜像会话被用户续聊过，task meta 已前进。
      { taskId: buildMirrorImportSessionId("peer-1"), title: "续聊过的", updatedAt: 800 },
    ],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.importedIntoView, 0);
  assert.equal(remote.createCalls.length, 0);
});

test("首个导入失败判为对端版本过低：整轮降级并中止，不再逐条重试", async () => {
  const remote = createMachine({
    localScope: [
      { taskId: "peer-1", updatedAt: 500 },
      { taskId: "peer-2", updatedAt: 400 },
    ],
    createSession: async () => {
      throw new Error("invalid importedHistory.source");
    },
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "unsupported");
  assert.match(result.error, /invalid importedHistory\.source/);
  assert.equal(result.importedIntoPeerLocal, 0);
  assert.equal(remote.createCalls.length, 1);
  assert.equal(local.readCalls.length, 0);
});

test("没有可读文本的会话不导入，不产生空会话", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", updatedAt: 500 }],
    readMessages: async () => [
      {
        info: {
          messageId: "m1",
          sessionId: "peer-1",
          role: "assistant",
          time: { created: 1 },
          agent: "mode-agent",
        },
        parts: [],
      },
    ],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 0);
  assert.equal(remote.createCalls.length, 0);
});

test("单条记录读取失败只跳过该条，其余继续", async () => {
  const remote = createMachine({
    localScope: [
      { taskId: "bad", updatedAt: 600 },
      { taskId: "ok", updatedAt: 500 },
    ],
    readMessages: async (taskId) => {
      if (taskId === "bad") {
        throw new Error("session not found");
      }
      return [textMessage("能读")];
    },
  });
  const local = createMachine({ localScope: [] });
  const warnings: string[] = [];

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
    onWarning: (message) => warnings.push(message),
  });

  assert.equal(result.status, "ready");
  assert.deepEqual(
    remote.createCalls.map((call) => call.sessionId),
    [buildMirrorImportSessionId("ok")],
  );
  assert.ok(warnings.some((message) => message.includes("bad")));
});

test("有记录但全部读取失败：报 degraded 而不是 ready，别把全面失败伪装成没东西可同步", async () => {
  const remote = createMachine({
    localScope: [
      { taskId: "peer-1", updatedAt: 600 },
      { taskId: "peer-2", updatedAt: 500 },
    ],
    // 对端 CLI 没有冷读能力时，每个冷会话都抛这个错——正是恒导入 0 的那次线上故障。
    readMessages: async () => {
      throw new Error("Session is not active: peer-1");
    },
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "degraded");
  assert.match(result.error, /Session is not active/);
  assert.equal(result.importedIntoView, 0);
  assert.equal(remote.createCalls.length, 0);
});

test("本地工作区不做任何镜像：来源与目标是同一个 scope，导入只会凭空造副本", async () => {
  const machine = createMachine({ localScope: [{ taskId: "mine-1", updatedAt: 700 }] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: machine.services,
    localServices: machine.services,
    isRemoteTarget: false,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 0);
  assert.equal(result.importedIntoPeerLocal, 0);
  assert.equal(machine.createCalls.length, 0);
  assert.equal(machine.listScopes.length, 0);
});

test("②方向覆盖本机在远端工作区发起的对话：来源取视图 scope，仍落到对端本地 scope", async () => {
  const remote = createMachine({
    localScope: [],
    // 本机连上远端工作区后在其中开的对话：落在对端机器、带本机 attach 身份。
    viewScope: [{ taskId: "mine-in-remote", title: "本机在远端工作区开的", updatedAt: 800 }],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoPeerLocal, 1);
  const [peerImport] = peerLocalCalls(remote.createCalls);
  assert.ok(peerImport, "本机在远端工作区的对话必须被导到对端本地 scope");
  assert.equal(peerImport.sessionId, buildMirrorImportSessionId("mine-in-remote"));
  assert.equal(peerImport.importedHistory?.originSessionId, "mine-in-remote");
  // 消息回它所在的机器读：这条落在远端机器上，不能拿本机 host 去读。
  assert.deepEqual(remote.readCalls, ["mine-in-remote"]);
  assert.deepEqual(local.readCalls, []);
});

/** 12 条来源会话，updatedAt = 100…1200（越大越新）。窗口用例都在这个量级上取，避开 10 条下限。 */
function windowRows(): Array<{ taskId: string; updatedAt: number }> {
  return Array.from({ length: 12 }, (_unused, index) => ({
    taskId: `t-${index}`,
    updatedAt: (index + 1) * 100,
  }));
}

/** 期望的导入序列：按 updatedAt 倒序取最新的前 count 条。 */
function expectedImportOrder(count: number): string[] {
  return Array.from({ length: count }, (_unused, index) =>
    buildMirrorImportSessionId(`t-${11 - index}`),
  );
}

test("同步窗口至少 10 条：侧栏默认只可见 5 条时也同步最近 10 条", async () => {
  const narrow = createMachine({ localScope: windowRows() });
  await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: narrow.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: createMachine({}).services,
    isRemoteTarget: true,
    windowSize: 5,
  });

  assert.deepEqual(
    viewDirectionCalls(narrow.createCalls).map((call) => call.sessionId),
    expectedImportOrder(10),
  );
});

test("同步窗口只处理最新的 N 条：窗口变大后更旧的记录才会被同步（对应侧栏「展开更多」）", async () => {
  const rows = windowRows();

  const narrow = createMachine({ localScope: rows });
  await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: narrow.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: createMachine({}).services,
    isRemoteTarget: true,
    windowSize: 10,
  });
  assert.deepEqual(
    viewDirectionCalls(narrow.createCalls).map((call) => call.sessionId),
    expectedImportOrder(10),
  );

  const wide = createMachine({ localScope: rows });
  await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: wide.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: createMachine({}).services,
    isRemoteTarget: true,
    windowSize: 12,
  });
  assert.deepEqual(
    viewDirectionCalls(wide.createCalls).map((call) => call.sessionId),
    expectedImportOrder(12),
  );
});

test("水位不前进时连消息都不读：空转轮不再按满负荷重读全部来源", async () => {
  // 目标 scope 已有同版本镜像会话 → 这条来源没有什么可同步的。
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", title: "对端会话", updatedAt: 400 }],
    viewScope: [
      { taskId: buildMirrorImportSessionId("peer-1"), title: "已是最新", updatedAt: 400 },
    ],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 0);
  // 读消息是整轮最贵的一步（单会话最多 200 条走 RPC），水位在读取之前就已知：
  // 「先读后判」会让每轮空转都付出满负荷代价，轮次因此永远跑不完。
  assert.deepEqual(remote.readCalls, []);
  assert.deepEqual(local.readCalls, []);
});

test("读取失败的来源进入退避：下一轮不重读，但本轮仍如实报 degraded", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", title: "读不出来的会话", updatedAt: 500 }],
    readMessages: async () => {
      // 线上形态：file part 的空 url 让整条响应过不了 wire 校验。
      throw new Error('ZodError: [{"path":["messages",67,"parts",1,"url"]}]');
    },
  });
  const local = createMachine({ localScope: [] });
  const readBackoff = createConversationMirrorReadBackoff();

  const first = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
    readBackoff,
  });
  assert.equal(first.status, "degraded");
  assert.equal(first.error.includes("parts"), true, "degraded 要带上失败原文，否则 UI 侧无法诊断");
  assert.deepEqual(remote.readCalls, ["peer-1"]);

  const second = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
    readBackoff,
  });
  // 退避期内不再重读同一条来源：否则每轮撞同一个错误、打同一段日志。
  assert.deepEqual(remote.readCalls, ["peer-1"]);
  // 但退避不能把「这条会话始终没同步上」洗成 ready。
  assert.equal(second.status, "degraded");
});

test("退避清掉后恢复读取，成功一次即把该来源移出退避", async () => {
  let failing = true;
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", title: "先失败后成功的会话", updatedAt: 500 }],
    readMessages: async () => {
      if (failing) {
        throw new Error("boom");
      }
      return [textMessage("内容-peer-1")];
    },
  });
  const local = createMachine({ localScope: [] });
  const readBackoff = createConversationMirrorReadBackoff();
  const params = {
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
    readBackoff,
  } as const;

  assert.equal((await runConversationMirrorSync(params)).status, "degraded");
  assert.deepEqual(remote.readCalls, ["peer-1"]);

  failing = false;
  // 手动重试语义：hook 的 refresh 先清退避，再跑一轮。
  readBackoff.reset();
  const retried = await runConversationMirrorSync(params);

  assert.equal(retried.status, "ready");
  assert.equal(retried.importedIntoView, 1);
  assert.deepEqual(remote.readCalls, ["peer-1", "peer-1"]);
  assert.equal(readBackoff.shouldSkip("peer-1"), undefined);
});

test("来源列表被镜像副本占满时逐级补页：真实来源仍会同步过来（不再只取固定一页）", async () => {
  // 40 条镜像副本排在最前（updatedAt 更新）——一台机器用久之后，对端本地 scope 头部的真实形态。
  // 旧的固定一页口径（maxRecords + windowSize = 30）在这里取不到任何真实来源，更旧的来源就永远同步不过来。
  const mirrorRows = Array.from({ length: 40 }, (_, index) => ({
    taskId: buildMirrorImportSessionId(`imported-${index}`),
    updatedAt: 10_000 - index,
  }));
  const remote = createMachine({
    localScope: [...mirrorRows, { taskId: "peer-real", title: "真实来源", updatedAt: 100 }],
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "ready");
  assert.equal(result.importedIntoView, 1);
  assert.ok(
    remote.createCalls.some((call) => call.sessionId === buildMirrorImportSessionId("peer-real")),
    "真实来源必须被导入，不能因为一页被镜像副本占满就漏掉",
  );
  const firstLimit = remote.requestedLimits[0] ?? 0;
  assert.equal(firstLimit, 30, "起始一页 = maxRecords(20) + windowSize(10)");
  assert.ok(
    remote.requestedLimits.some((limit) => limit > firstLimit),
    "页内可用来源不足 windowSize 时必须补页",
  );
});

test("导入失败是暂时性错误（超时 / 网络）时只跳过该条并报 degraded，不误判为对端版本过低", async () => {
  const remote = createMachine({
    localScope: [{ taskId: "peer-1", updatedAt: 500 }],
    createSession: async () => {
      throw new Error("mode-protocol request timed out after 180000ms");
    },
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  // 一次抖动不该把整轮方向判死，更不该给出「对端版本过低」这个错误理由。
  assert.equal(result.status, "degraded");
  assert.match(result.error, /timed out/);
});

test("对端以 -32602 拒绝导入载荷时判为版本过低（即使错误文案没带字段名）", async () => {
  const remote = createMachine({
    localScope: [
      { taskId: "peer-1", updatedAt: 500 },
      { taskId: "peer-2", updatedAt: 400 },
    ],
    createSession: async () => {
      throw Object.assign(new Error("Invalid params"), { code: -32602 });
    },
  });
  const local = createMachine({ localScope: [] });

  const result = await runConversationMirrorSync({
    workspacePath: WORKSPACE_PATH,
    viewServices: remote.services,
    viewWorkspaceIdentity: VIEW_IDENTITY,
    localServices: local.services,
    isRemoteTarget: true,
  });

  assert.equal(result.status, "unsupported");
  assert.equal(remote.createCalls.length, 1, "首个失败即中止该方向，不再逐条重撞同一个错误");
});
