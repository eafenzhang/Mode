import assert from "node:assert/strict";
import test from "node:test";
import { buildMirrorImportSessionId } from "@mode/shared/conversation-mirror";
import { runConversationMirrorSync } from "../src/lib/conversationMirrorSync.js";

/**
 * 远程工作区对话记录镜像的编排回归（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 夹具按真实拓扑搭两台机器：远端机的「本地 scope」既是①方向的来源、又是②方向的目标，
 * 本机只提供②方向的来源。两边共用一份服务桩会让来源与目标重合，掩盖方向接错的问题。
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

  const modeTaskService = {
    async listTaskList(query: {
      workspaceScopes: Array<{ workspacePath: string; workspaceIdentity?: string }>;
    }) {
      const scope = query.workspaceScopes[0] ?? { workspacePath: WORKSPACE_PATH };
      listScopes.push(scope);
      const rows = scope.workspaceIdentity ? (params.viewScope ?? []) : (params.localScope ?? []);
      return {
        items: rows.map((row) => ({
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
        hasMore: false,
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
