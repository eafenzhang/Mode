/**
 * Mode Agent 反向配对（docs/specs/legacy-windows-remote-agent.md）：
 * 被控端拿桌面生成的 LAN 配对码登记自己；桌面端验证码、探测 agent /health，
 * 并把 agent 写成一个用户级 HTTP MCP server。
 */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import test from "node:test";

import {
  IMcpSyncService,
  ServiceCollection,
  type ICredentialService,
} from "@mode/services";
import type { SaveCliMcpToUserDirectoryRequest } from "@mode/shared";

import { startLanAccessServer } from "../src/host/lanAccessServer.js";

function createFakeCredentials(): ICredentialService {
  const store = new Map<string, string>();
  return {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
}

test("被控端用桌面配对码反向登记：写 MCP、消费配对码、探测不可达 agent", async () => {
  const saved: SaveCliMcpToUserDirectoryRequest[] = [];
  const services = new ServiceCollection();
  services.register(IMcpSyncService, {
    saveMcpToUserDirectory: async (payload: SaveCliMcpToUserDirectoryRequest) => {
      saved.push(payload);
    },
  } as unknown as IMcpSyncService);

  const agentServer = createServer((request, response) => {
    if (request.url === "/health") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end('{"ok":true}');
      return;
    }
    response.writeHead(404);
    response.end();
  });
  await new Promise<void>((resolve) => agentServer.listen(0, "127.0.0.1", resolve));
  const agentPort = (agentServer.address() as AddressInfo).port;

  const lan = await startLanAccessServer({
    services,
    credentials: createFakeCredentials(),
    getWorkspaces: async () => [],
    appVersion: "test",
    machineName: "enrollment-test",
    port: 0,
  });
  try {
    const state = lan.createPairCode();
    const code = state.pairCode?.code;
    assert.ok(code, "desktop must create a pair code");
    const port = state.port;
    assert.ok(port && port > 0);

    const body = {
      code,
      agentId: "abc123",
      name: "OFFICE-PC",
      host: "127.0.0.1",
      port: agentPort,
      token: "t".repeat(32),
      version: "0.1.0",
      platform: "windows",
    };
    const response = await fetch(`http://127.0.0.1:${port}/api/remote-agent/enroll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), { ok: true, serverName: "mode-agent-abc123" });
    assert.equal(saved.length, 1);
    const payload = saved[0]!;
    assert.equal(payload.action, "upsert");
    assert.equal(payload.source, "zcodeagentmcp");
    assert.equal(payload.name, "mode-agent-abc123");
    assert.equal(payload.config?.type, "http");
    assert.equal(payload.config?.url, `http://127.0.0.1:${agentPort}/mcp`);
    assert.deepEqual(payload.config?.headers, { Authorization: `Bearer ${"t".repeat(32)}` });
    assert.equal((payload.config as { modeAgent?: { name?: string } }).modeAgent?.name, "OFFICE-PC");

    const replay = await fetch(`http://127.0.0.1:${port}/api/remote-agent/enroll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    assert.equal(replay.status, 401);
    assert.equal(((await replay.json()) as { error?: string }).error, "invalid_pair_code");
    assert.equal(saved.length, 1, "replayed code must not write another MCP server");

    const unreachable = await fetch(`http://127.0.0.1:${port}/api/remote-agent/enroll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        ...body,
        code: lan.createPairCode().pairCode?.code,
        port: 1,
      }),
    });
    assert.equal(unreachable.status, 401);
    assert.equal(((await unreachable.json()) as { error?: string }).error, "agent_unreachable");
    assert.equal(saved.length, 1, "unreachable agent must not be saved");
  } finally {
    await lan.dispose();
    await new Promise<void>((resolve, reject) =>
      agentServer.close((error) => (error ? reject(error) : resolve())),
    );
  }
});
