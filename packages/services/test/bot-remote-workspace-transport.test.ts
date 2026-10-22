import assert from "node:assert/strict";
import test from "node:test";

import type { RemoteTarget } from "@mode/shared";

import {
  createBotRemoteWorkspaceService,
  type BotRemoteWorkspaceTransport,
  type BotRemoteWorkspaceTransportTarget,
} from "../src/bots/botRemoteWorkspaceBridge.js";

// 迁移计划 §5.4（botRemote.reconnected）：Tauri Host 里 registry 与 BotsService 同进程，
// Bot 桥用直连传输替代 parentPort 转发。这里把三个委派方法与缓存/dispose 语义钉住。

const lanTarget: RemoteTarget = { kind: "lan", host: "127.0.0.1", port: 45880 };
const workspacePath = "/w";
const workspaceIdentity = "remote:lan:127.0.0.1:45880:/w";

function fakeDeps() {
  const settings = {
    lastWorkspaceSession: [
      {
        kind: "remote" as const,
        workspacePath,
        workspaceIdentity,
        target: lanTarget,
        lastOpenedAt: 1,
        lastConnectionStatus: "connected" as const,
      },
    ],
    askUserQuestionAutoResolutionEnabled: true,
    modelIoFullRetentionEnabled: false,
  };
  const settingService = {
    async get() {
      return settings;
    },
  };
  const credentialService = {
    async load() {
      return undefined;
    },
  };
  const runtimeServices = {
    modeAgentService: {
      async syncAppRuntimePreferences() {
        return undefined;
      },
    },
    modeTaskService: {},
    modeSessionService: {},
    modelSelectionService: {
      async getView() {
        return { preferredSelection: null };
      },
    },
  };
  return { settingService, credentialService, runtimeServices };
}

test("没有 parentPort 也没有直连传输时不创建 Bot 桥（沿用旧语义）", () => {
  const { settingService, credentialService } = fakeDeps();
  assert.equal(
    createBotRemoteWorkspaceService({ settingService, credentialService } as never),
    undefined,
  );
});

test("isConnected 优先问直连传输；查询失败回退本地标记", async () => {
  const { settingService, credentialService } = fakeDeps();
  const queries: BotRemoteWorkspaceTransportTarget[] = [];
  let answer: boolean | null = null;
  const transport: BotRemoteWorkspaceTransport = {
    async queryConnectionStatus(target) {
      queries.push(target);
      return answer;
    },
    async reconnect() {
      return { ok: true, sessionId: "session-1" };
    },
    async createRuntimePort() {
      throw new Error("not used");
    },
  };
  const service = createBotRemoteWorkspaceService({
    settingService,
    credentialService,
    transport,
  } as never);
  assert.ok(service);

  // 查询返回 null（失败）：走本地 Set（初始为空）。
  assert.equal(await service.isConnected({ workspacePath, workspaceIdentity }), false);
  assert.equal(queries.length, 1);
  assert.deepEqual(queries[0]?.target, lanTarget, "target 由 settings 快照解析后交给传输");

  // 重连成功写入本地标记；随后查询失败仍应命中本地标记。
  await service.ensureConnected({ workspacePath, workspaceIdentity });
  answer = null;
  assert.equal(await service.isConnected({ workspacePath, workspaceIdentity }), true);

  // 查询返回 false：本地标记被清理（远端已断开）。
  answer = false;
  assert.equal(await service.isConnected({ workspacePath, workspaceIdentity }), false);
});

test("ensureConnected 委派直连重连：成功写入标记，失败把错误原样带回", async () => {
  const { settingService, credentialService } = fakeDeps();
  const reconnects: BotRemoteWorkspaceTransportTarget[] = [];
  let next: { ok: boolean; sessionId?: string; error?: string } = {
    ok: false,
    error: "远端 workspace 重连超时。",
  };
  const transport: BotRemoteWorkspaceTransport = {
    async queryConnectionStatus() {
      return null;
    },
    async reconnect(target) {
      reconnects.push(target);
      return next;
    },
    async createRuntimePort() {
      throw new Error("not used");
    },
  };
  const service = createBotRemoteWorkspaceService({
    settingService,
    credentialService,
    transport,
  } as never);
  assert.ok(service);

  assert.deepEqual(await service.ensureConnected({ workspacePath, workspaceIdentity }), {
    ok: false,
    message: "远端 workspace 重连超时。",
  });
  assert.equal(reconnects.length, 1);

  next = { ok: true, sessionId: "session-1" };
  assert.deepEqual(await service.ensureConnected({ workspacePath, workspaceIdentity }), {
    ok: true,
  });
});

test("runtime port 走直连传输：缓存 services、同步偏好、dispose 关闭端口", async () => {
  const { settingService, credentialService, runtimeServices } = fakeDeps();
  const targets: BotRemoteWorkspaceTransportTarget[] = [];
  let disposed = 0;
  let preferenceSyncs = 0;
  runtimeServices.modeAgentService.syncAppRuntimePreferences = async () => {
    preferenceSyncs += 1;
  };
  const transport: BotRemoteWorkspaceTransport = {
    async queryConnectionStatus() {
      return true;
    },
    async reconnect() {
      return { ok: true, sessionId: "session-1" };
    },
    async createRuntimePort(target) {
      targets.push(target);
      return {
        ok: true,
        port: { fake: "port" },
        dispose: () => {
          disposed += 1;
        },
      };
    },
  };
  const service = createBotRemoteWorkspaceService({
    settingService,
    credentialService,
    transport,
    createRuntimeServicesFromPort: () => runtimeServices as never,
  } as never);
  assert.ok(service);

  const first = await service.getModeTaskService({ workspacePath, workspaceIdentity });
  assert.equal(first, runtimeServices.modeTaskService);
  assert.equal(targets.length, 1, "建 runtime port 必须携带解析后的 target");
  assert.deepEqual(targets[0]?.target, lanTarget);
  assert.equal(preferenceSyncs, 1, "远端 Agent 必须先同步 App Runtime 偏好");

  // 第二次命中缓存：不再建端口。
  await service.getModeTaskService({ workspacePath, workspaceIdentity });
  assert.equal(targets.length, 1);
  assert.equal(preferenceSyncs, 1);

  service.dispose();
  assert.equal(disposed, 1, "dispose 必须关闭直连传输建的端口");
});

test("runtime port 失败时抛错，不缓存半个 services", async () => {
  const { settingService, credentialService } = fakeDeps();
  const transport: BotRemoteWorkspaceTransport = {
    async queryConnectionStatus() {
      return false;
    },
    async reconnect() {
      return { ok: false, error: "boom" };
    },
    async createRuntimePort() {
      return { ok: false, error: "远端 workspace 尚未连接或未绑定上下文。" };
    },
  };
  let created = 0;
  const service = createBotRemoteWorkspaceService({
    settingService,
    credentialService,
    transport,
    createRuntimeServicesFromPort: () => {
      created += 1;
      return {} as never;
    },
  } as never);
  assert.ok(service);

  await assert.rejects(
    service.getModeTaskService({ workspacePath, workspaceIdentity }),
    /尚未连接或未绑定上下文/u,
  );
  assert.equal(created, 0, "端口失败时不得先建 runtime services");
});
