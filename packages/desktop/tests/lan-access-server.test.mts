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

import { ServiceCollection, type ICredentialService } from "@mode/services";

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
