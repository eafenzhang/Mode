import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import type { ServiceCollection } from "@mode/services";
import { serverRemoteInfoSchema } from "@mode/shared";
import { createHttpServer } from "../src/http.js";

// 远程桌面设备能力位（spec: docs/specs/lan-remote-desktop.md 产品规则 5）：
// 只有被控 host（desktop LAN access server）显式声明时才发键；缺键 = 旧对端不支持。
// 组合口径（win32 && 被控开关 && kill-switch）在 lanAccessServer 侧按纯函数断言。

async function fetchServerInfo(options: { remoteDesktopCapability?: boolean | (() => boolean) }) {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    ...(options.remoteDesktopCapability !== undefined
      ? { remoteDesktopCapability: options.remoteDesktopCapability }
      : {}),
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/server-info`);
    assert.equal(response.status, 200);
    const parsed = serverRemoteInfoSchema.safeParse(await response.json());
    assert.equal(parsed.success, true, "server-info 必须通过共享 schema");
    return parsed.success ? parsed.data : undefined;
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

async function startServerWithCapability(capability: () => boolean) {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    remoteDesktopCapability: capability,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  return {
    read: async (): Promise<boolean | undefined> => {
      const response = await fetch(`http://127.0.0.1:${port}/api/server-info`);
      const parsed = serverRemoteInfoSchema.parse(await response.json());
      return parsed.capabilities.remoteDesktop;
    },
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}

test("声明 remoteDesktopCapability:true 时 capabilities.remoteDesktop 为 true", async () => {
  const info = await fetchServerInfo({ remoteDesktopCapability: true });
  assert.equal(info?.capabilities.remoteDesktop, true);
});

test("声明 remoteDesktopCapability:false 时键存在且为 false", async () => {
  const info = await fetchServerInfo({ remoteDesktopCapability: false });
  assert.equal(info?.capabilities.remoteDesktop, false);
});

test("未声明时键缺省（旧对端与其余消费方行为不变）", async () => {
  const info = await fetchServerInfo({});
  assert.equal(info?.capabilities.remoteDesktop, undefined);
});

test("函数形式：按每次握手现取（被控开关是可变设置，打开后无需重启监听）", async () => {
  let enabled = false;
  const handle = await startServerWithCapability(() => enabled);
  try {
    assert.equal(await handle.read(), false);
    enabled = true;
    assert.equal(await handle.read(), true, "开关打开后能力位必须立即可见");
    enabled = false;
    assert.equal(await handle.read(), false, "开关关闭后能力位必须立即收回");
  } finally {
    await handle.close();
  }
});
