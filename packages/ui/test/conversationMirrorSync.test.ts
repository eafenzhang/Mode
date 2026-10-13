import { describe, expect, it, vi } from "vitest";
import { buildMirrorImportSessionId } from "@mode/shared/conversation-mirror";
import { runConversationMirrorSync } from "../src/lib/conversationMirrorSync.js";

/**
 * 一轮对话记录镜像同步的编排回归（docs/specs/remote-workspace-conversation-mirror.md）：
 * 镜像记录以 `session/create` + `importedHistory` 导入成真实会话，两个方向各落一个 scope；
 * 未变更的不重复导入；对端版本过低时降级；单条失败不影响其余。
 */

const WORKSPACE_PATH = "D:/proj";
const VIEW_IDENTITY = "remote:lan:192.168.1.20:45880:D:/proj";

function textMessage(text: string) {
  return {
    info: {
      messageId: `m-${text}`,
      sessionId: "s1",
      role: "user",
      time: { created: 1 },
      agent: "mode-agent",
    },
    parts: [{ partId: `p-${text}`, sessionId: "s1", messageId: `m-${text}`, type: "text", text }],
  } as never;
}

interface TaskListEntry {
  taskId: string;
  title?: string;
  updatedAt: number;
}

function createServices(params: {
  /** 该机器上「不带 identity」的本地 scope 行。 */
  localScope?: TaskListEntry[];
  /** 该机器上「带 identity」（视图 scope）的行。 */
  viewScope?: TaskListEntry[];
  readMessages?: (taskId: string) => Promise<unknown>;
  createSession?: (params: unknown) => Promise<unknown>;
}) {
  const createCalls: Array<Record<string, unknown>> = [];

  const services = {
    modeTaskService: {
      listTaskList: vi.fn(
        async (query: {
          workspaceScopes: Array<{ workspacePath: string; workspaceIdentity?: string }>;
        }) => {
          const scope = query.workspaceScopes[0] ?? { workspacePath: WORKSPACE_PATH };
          const rows = scope.workspaceIdentity
            ? (params.viewScope ?? [])
            : (params.localScope ?? []);
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
      ),
    },
    modeSessionService: {
      readSessionMessages: vi.fn(async ({ sessionId }: { sessionId: string }) =>
        params.readMessages
          ? await params.readMessages(sessionId)
          : [textMessage(`内容-${sessionId}`)],
      ),
      createSession: vi.fn(async (createParams: Record<string, unknown>) => {
        createCalls.push(createParams);
        if (params.createSession) {
          return await params.createSession(createParams);
        }
        return {};
      }),
    },
  } as never;

  return { services, createCalls };
}

describe("runConversationMirrorSync", () => {
  it("两个方向各自导入：对端记录进视图 scope（带 identity），本机记录进对端本地 scope（不带 identity）", async () => {
    const remoteMachine = createServices({
      localScope: [{ taskId: "peer-1", title: "对端会话", updatedAt: 500 }],
      viewScope: [],
    });
    const localMachine = createServices({
      localScope: [{ taskId: "mine-1", title: "本机会话", updatedAt: 700 }],
    });

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: localMachine.services,
      isRemoteTarget: true,
    });

    expect(result.status).toBe("ready");
    expect(result.importedIntoView).toBe(1);
    expect(result.importedIntoPeerLocal).toBe(1);

    const viewImport = remoteMachine.createCalls.find(
      (call) => call.sessionId === buildMirrorImportSessionId("peer-1"),
    );
    expect(viewImport).toMatchObject({
      workspacePath: WORKSPACE_PATH,
      workspaceIdentity: VIEW_IDENTITY,
      persistence: "immediate",
    });
    expect(viewImport?.importedHistory).toMatchObject({
      source: "remoteMirror",
      originSessionId: "peer-1",
    });

    const peerLocalImport = remoteMachine.createCalls.find(
      (call) => call.sessionId === buildMirrorImportSessionId("mine-1"),
    );
    expect(peerLocalImport).toMatchObject({
      workspacePath: WORKSPACE_PATH,
      persistence: "immediate",
    });
    // 反向导入不得带 identity：它要落在对端自己的本地工作区。
    expect(peerLocalImport && "workspaceIdentity" in peerLocalImport).toBe(false);
  });

  it("目标 scope 已有同版本会话（含用户续聊后更新的）时不重复导入", async () => {
    const remoteMachine = createServices({
      localScope: [{ taskId: "peer-1", title: "对端会话", updatedAt: 500 }],
      viewScope: [
        { taskId: buildMirrorImportSessionId("peer-1"), title: "已导入", updatedAt: 900 },
      ],
    });
    const localMachine = createServices({ localScope: [] });

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: localMachine.services,
      isRemoteTarget: true,
    });

    expect(result.importedIntoView).toBe(0);
    expect(remoteMachine.createCalls).toHaveLength(0);
  });

  it("来源里的镜像副本（mirror- 前缀）不再被转发到反向 scope", async () => {
    const remoteMachine = createServices({
      localScope: [
        { taskId: "peer-1", title: "对端会话", updatedAt: 500 },
        {
          taskId: buildMirrorImportSessionId("mine-1"),
          title: "我上次导入过去的副本",
          updatedAt: 900,
        },
      ],
    });
    // 本机这一侧没有自己的记录，隔离出①方向的行为。
    const localMachine = createServices({ localScope: [] });

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: localMachine.services,
      isRemoteTarget: true,
    });

    // 只导入对端真实会话一次；对端列表里那份 mirror- 副本不会被当成来源（否则会出现 mirror-mirror-…）。
    expect(result.importedIntoView).toBe(1);
    expect(remoteMachine.createCalls.map((call) => call.sessionId)).toEqual([
      buildMirrorImportSessionId("peer-1"),
    ]);
  });

  it("首个导入失败即判为对端版本过低：本轮中止，不再继续尝试", async () => {
    const remoteMachine = createServices({
      localScope: [
        { taskId: "peer-1", updatedAt: 500 },
        { taskId: "peer-2", updatedAt: 400 },
      ],
      createSession: async () => {
        throw new Error("invalid importedHistory.source");
      },
    });

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: remoteMachine.services,
      isRemoteTarget: true,
    });

    expect(result.status).toBe("unsupported");
    expect(result.error).toContain("invalid importedHistory.source");
    expect(remoteMachine.createCalls).toHaveLength(1);
  });

  it("没有可读文本的会话不导入（不产生空会话）", async () => {
    const remoteMachine = createServices({
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

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: remoteMachine.services,
      isRemoteTarget: true,
    });

    expect(result.status).toBe("ready");
    expect(remoteMachine.createCalls).toHaveLength(0);
  });

  it("单条记录读取失败只跳过该条", async () => {
    const remoteMachine = createServices({
      localScope: [
        { taskId: "bad", updatedAt: 600 },
        { taskId: "ok", updatedAt: 500 },
      ],
      readMessages: async (taskId: string) => {
        if (taskId === "bad") {
          throw new Error("session not found");
        }
        return [textMessage("能读")];
      },
    });
    const localMachine = createServices({ localScope: [] });
    const warnings: string[] = [];

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: remoteMachine.services,
      viewWorkspaceIdentity: VIEW_IDENTITY,
      localServices: localMachine.services,
      isRemoteTarget: true,
      onWarning: (message) => warnings.push(message),
    });

    expect(result.status).toBe("ready");
    expect(remoteMachine.createCalls.map((call) => call.sessionId)).toEqual([
      buildMirrorImportSessionId("ok"),
    ]);
    expect(warnings.some((message) => message.includes("bad"))).toBe(true);
  });

  it("本地工作区不做任何镜像（来源与目标是同一个 scope）", async () => {
    const machine = createServices({
      localScope: [{ taskId: "mine-1", updatedAt: 700 }],
    });

    const result = await runConversationMirrorSync({
      workspacePath: WORKSPACE_PATH,
      viewServices: machine.services,
      localServices: machine.services,
      isRemoteTarget: false,
    });

    expect(result.status).toBe("ready");
    expect(machine.createCalls).toHaveLength(0);
  });
});
