import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import type { ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";

/** 桌面 LAN 服务挂载 Web 静态资源 + 配对 Cookie（docs/specs/runtime-ui-acceptance-fixes.md §4）。 */
test("LAN 服务静态首页免令牌，配对响应写 mode_lite_token Cookie", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-lan-web-"));
  await mkdir(join(root, "assets"), { recursive: true });
  await writeFile(join(root, "index.html"), "<!doctype html><title>Mode Web</title>");
  await writeFile(join(root, "assets", "app.js"), "console.log('mode')");
  const services = {
    getOptional: () => undefined,
    exposeOnChannelServer: () => undefined,
  } as unknown as ServiceCollection;

  const server = createHttpServer(services, 0, {
    host: "127.0.0.1",
    authRequired: true,
    authToken: "unused",
    verifyAuthToken: (token) => token === "paired-token",
    lanPairing: {
      redeem: async (request) =>
        request.code.trim().toUpperCase() === "A1B2C3"
          ? { token: "paired-token", serverId: "server-1", name: "host" }
          : null,
    },
    serverId: "server-1",
    name: "host",
    workspaces: [],
    staticRoot: root,
    spaFallback: true,
  });

  try {
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
    const address = server.address() as AddressInfo;
    const base = `http://127.0.0.1:${address.port}`;

    const page = await fetch(`${base}/`);
    assert.equal(page.status, 200);
    assert.match(await page.text(), /Mode Web/);

    const asset = await fetch(`${base}/assets/app.js`);
    assert.equal(asset.status, 200);
    assert.match(await asset.text(), /mode/);

    const unauthorized = await fetch(`${base}/api/server-info`);
    assert.equal(unauthorized.status, 401);

    const paired = await fetch(`${base}/api/lan/pair`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code: "a1b2c3", label: "Web browser" }),
    });
    assert.equal(paired.status, 200);
    const setCookie = paired.headers.get("set-cookie") ?? "";
    assert.match(setCookie, /mode_lite_token=paired-token/u);
    assert.match(setCookie, /HttpOnly/u);

    const info = await fetch(`${base}/api/server-info`, {
      headers: { cookie: "mode_lite_token=paired-token" },
    });
    assert.equal(info.status, 200);
    const body = (await info.json()) as { serverId?: string };
    assert.equal(body.serverId, "server-1");

    // 浏览器配对后的真实路径：Cookie 鉴权的 /ws 升级 + ChannelServer Initialize 帧。
    const ws = new WebSocket(`ws://127.0.0.1:${address.port}/ws`, {
      headers: { cookie: "mode_lite_token=paired-token" },
    });
    try {
      const firstFrame = await new Promise<Buffer>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("WS 首帧超时")), 3_000);
        ws.once("message", (data) => {
          clearTimeout(timer);
          resolve(Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer));
        });
        ws.once("error", (error) => {
          clearTimeout(timer);
          reject(error);
        });
      });
      assert.ok(firstFrame.byteLength > 0, "ChannelServer 必须下发 Initialize 帧");
    } finally {
      ws.close();
    }
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(root, { recursive: true, force: true });
  }
});

test("普通 /ws 连接通过 onWebSessionChange 上报 +1 / -1", async () => {
  const services = {
    getOptional: () => undefined,
    exposeOnChannelServer: () => undefined,
  } as unknown as ServiceCollection;
  const deltas: Array<1 | -1> = [];
  const server = createHttpServer(services, 0, {
    host: "127.0.0.1",
    authRequired: true,
    authToken: "unused",
    verifyAuthToken: (token) => token === "paired-token",
    serverId: "server-1",
    name: "host",
    workspaces: [],
    onWebSessionChange: (delta) => deltas.push(delta),
  });

  const waitFor = async (predicate: () => boolean, label: string) => {
    const deadline = Date.now() + 3_000;
    while (!predicate() && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    assert.ok(predicate(), label);
  };

  try {
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
    const address = server.address() as AddressInfo;
    const ws = new WebSocket(`ws://127.0.0.1:${address.port}/ws`, {
      headers: { cookie: "mode_lite_token=paired-token" },
    });
    try {
      await waitFor(() => deltas.length >= 1, "连接建立必须上报 +1");
      assert.deepEqual(deltas, [1]);
      ws.close();
      await waitFor(() => deltas.length >= 2, "连接关闭必须上报 -1");
      assert.deepEqual(deltas, [1, -1]);
    } finally {
      ws.terminate();
    }
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

test("staticIndexFile 可让 LAN 服务用 web.html 作为首页与 SPA 回退", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-lan-web-index-"));
  await mkdir(join(root, "assets"), { recursive: true });
  await writeFile(join(root, "index.html"), "<!doctype html><title>Desktop</title>");
  await writeFile(join(root, "web.html"), "<!doctype html><title>Browser</title>");
  await writeFile(join(root, "assets", "app.js"), "console.log('mode')");
  const services = {
    getOptional: () => undefined,
    exposeOnChannelServer: () => undefined,
  } as unknown as ServiceCollection;

  const server = createHttpServer(services, 0, {
    host: "127.0.0.1",
    authRequired: true,
    authToken: "unused",
    verifyAuthToken: () => false,
    serverId: "server-1",
    name: "host",
    workspaces: [],
    staticRoot: root,
    spaFallback: true,
    staticIndexFile: "web.html",
  });

  try {
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
    const address = server.address() as AddressInfo;
    const base = `http://127.0.0.1:${address.port}`;

    const home = await fetch(`${base}/`);
    assert.equal(home.status, 200);
    assert.match(await home.text(), /Browser/);

    const spa = await fetch(`${base}/share/callback`);
    assert.equal(spa.status, 200);
    assert.match(await spa.text(), /Browser/);

    const asset = await fetch(`${base}/assets/app.js`);
    assert.equal(asset.status, 200);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(root, { recursive: true, force: true });
  }
});
