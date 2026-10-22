/**
 * LAN 监听失败语义（2026-10-20 回归）。
 *
 * 背景：`startLanAccessServer` 直接使用 `@hono/node-server` 返回的 Node http.Server，
 * 端口被占用时 `error` 事件没有监听者，会作为未捕获异常杀死整个 Host；壳只能报
 * `transport_closed`，用户看到「数据准备进程意外退出」。本用例守住两件事：
 * 1. 端口冲突必须以 rejection 返回（调用方按「本次 LAN 不可用」处理）；
 * 2. 端口 0 的正常监听仍能拿到实际端口，并能 dispose。
 */
import assert from "node:assert/strict";
import { createServer } from "node:net";
import type { AddressInfo } from "node:net";
import test from "node:test";
import { WebSocket } from "ws";

import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  buildLanAccessClientTokenKey,
} from "@mode/shared";
import { ServiceCollection, type ICredentialService } from "@mode/services";

import { startLanAccessServer } from "../src/host/lanAccessServer.js";

function createFakeCredentialsStore(): {
  credentials: ICredentialService;
  store: Map<string, string>;
} {
  const store = new Map<string, string>();
  const credentials = {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
  return { credentials, store };
}

function createFakeCredentials(): ICredentialService {
  return createFakeCredentialsStore().credentials;
}

function startParams(port: number) {
  return {
    services: new ServiceCollection(),
    credentials: createFakeCredentials(),
    getWorkspaces: async () => [],
    appVersion: "test",
    machineName: "lan-access-server-test",
    port,
  };
}

test("端口被占用：startLanAccessServer 以 rejection 返回，不产生未捕获 error", async () => {
  const blocker = createServer();
  await new Promise<void>((resolve, reject) => {
    blocker.once("error", reject);
    blocker.listen(0, "0.0.0.0", () => resolve());
  });
  const blockedPort = (blocker.address() as AddressInfo).port;
  try {
    await assert.rejects(
      async () => await startLanAccessServer(startParams(blockedPort)),
      (error: unknown) => {
        assert.ok(error instanceof Error);
        assert.match(error.message, /局域网端口/u);
        assert.match(error.message, /监听失败/u);
        return true;
      },
    );
  } finally {
    await new Promise<void>((resolve, reject) => {
      blocker.close((error) => (error ? reject(error) : resolve()));
    });
  }
});

test("端口 0：正常监听返回实际端口，dispose 后不再保留监听", async () => {
  const handle = await startLanAccessServer(startParams(0));
  try {
    const state = handle.getState();
    assert.equal(state.enabled, true);
    assert.ok(state.port !== null && state.port > 0, "必须返回系统分配的真实端口");
  } finally {
    await handle.dispose();
  }
});

test("在线 web 会话计入 LanAccessState.activeWebSessions 并触发状态回调", async () => {
  let changes = 0;
  const { credentials, store } = createFakeCredentialsStore();
  store.set(
    LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
    JSON.stringify([
      { id: "client-1", label: "Web browser", createdAt: Date.now(), lastUsedAt: null },
    ]),
  );
  store.set(buildLanAccessClientTokenKey("client-1"), "client-token");
  const handle = await startLanAccessServer({
    ...startParams(0),
    credentials,
    onWebSessionChange: () => {
      changes += 1;
    },
  });
  try {
    const port = handle.getState().port;
    assert.ok(port && port > 0);
    const ws = new WebSocket(`ws://127.0.0.1:${port}/ws?token=client-token`);
    try {
      // 必须先等客户端握手完成：服务端 onOpen 可能先于客户端 OPEN，
      // 此时直接 ws.close() 会抛 "closed before the connection was established"。
      await new Promise<void>((resolve, reject) => {
        ws.once("open", () => resolve());
        ws.once("error", reject);
      });
      const openDeadline = Date.now() + 3_000;
      while (handle.getState().activeWebSessions !== 1 && Date.now() < openDeadline) {
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      assert.equal(handle.getState().activeWebSessions, 1, "连接建立后应计为在线");
      assert.ok(changes >= 1, "状态变化必须通知所有者刷新快照");

      await new Promise<void>((resolve) => {
        ws.once("close", () => resolve());
        ws.close();
      });
      const closeDeadline = Date.now() + 3_000;
      while (handle.getState().activeWebSessions !== 0 && Date.now() < closeDeadline) {
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      assert.equal(handle.getState().activeWebSessions, 0, "连接关闭后应回收");
    } finally {
      ws.terminate();
    }
  } finally {
    await handle.dispose();
  }
});

test("内置中继：生成稳定 tunnelId、启动隧道并在配对/server-info 下发 publicRelay", async () => {
  const started: Array<{
    relayUrl: string;
    token: string;
    tunnelId: string;
    localBaseUrl: string;
  }> = [];
  let closed = 0;
  const handle = await startLanAccessServer({
    ...startParams(0),
    relay: { baseUrl: "https://relay.example.com", token: "relay-token" },
    startTunnel: (options) => {
      started.push({
        relayUrl: options.relayUrl,
        token: options.token,
        tunnelId: options.tunnelId,
        localBaseUrl: options.localBaseUrl,
      });
      return {
        close: () => {
          closed += 1;
        },
      };
    },
  });
  try {
    const state = handle.getState();
    assert.equal(state.enabled, true);
    assert.ok(state.port !== null && state.port > 0);
    assert.equal(started.length, 1);
    const tunnel = started[0];
    assert.ok(tunnel);
    assert.equal(tunnel.relayUrl, "wss://relay.example.com/_tunnel");
    assert.equal(tunnel.token, "relay-token");
    assert.equal(tunnel.localBaseUrl, `http://127.0.0.1:${state.port}`);
    assert.match(tunnel.tunnelId, /^[A-Za-z0-9_-]{16,128}$/u);
    assert.equal(state.publicRelayUrl, `https://relay.example.com/d/${tunnel.tunnelId}`);

    const pairCode = handle.createPairCode().pairCode;
    assert.ok(pairCode);
    const pairResponse = await fetch(`http://127.0.0.1:${state.port}/api/lan/pair`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code: pairCode.code }),
    });
    assert.equal(pairResponse.status, 200);
    const paired = (await pairResponse.json()) as {
      token?: string;
      publicRelay?: { baseUrl?: string; tunnelId?: string };
    };
    assert.equal(paired.publicRelay?.baseUrl, "https://relay.example.com");
    assert.equal(paired.publicRelay?.tunnelId, tunnel.tunnelId);
    assert.ok(paired.token);

    const infoResponse = await fetch(
      `http://127.0.0.1:${state.port}/api/server-info?token=${encodeURIComponent(paired.token ?? "")}`,
    );
    assert.equal(infoResponse.status, 200);
    const info = (await infoResponse.json()) as {
      publicRelay?: { baseUrl?: string; tunnelId?: string };
    };
    assert.equal(info.publicRelay?.tunnelId, tunnel.tunnelId);
  } finally {
    await handle.dispose();
  }
  assert.equal(closed, 1, "dispose 必须关闭中继隧道");
});

test("未配置中继 token：不启动隧道，状态保持 LAN-only", async () => {
  let starts = 0;
  const handle = await startLanAccessServer({
    ...startParams(0),
    relay: { baseUrl: "https://relay.example.com", token: "" },
    startTunnel: () => {
      starts += 1;
      return { close: () => undefined };
    },
  });
  try {
    assert.equal(starts, 0);
    assert.equal(handle.getState().publicRelayUrl, null);
  } finally {
    await handle.dispose();
  }
});
