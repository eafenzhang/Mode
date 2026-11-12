/* oxlint-disable eslint(max-lines) -- 验收脚本按真人操作顺序线性展开；拆分会割裂步骤、断言与截图上下文。 */
/**
 * 远程控制 v3 Web 端本地模拟验收（人工触发，不进 CI）。
 *
 * 假 GitHub Device Flow + 两套真 remote-server/Host + 一个真实的 Host 本机 LAN 服务，
 * 用本机 Chrome 操作 /v3：登录、目录、加密 RPC（P2P/Relay）、工作区隧道、
 * 大响应分片、多账号切换、Host 上下线、手机视口与错误路径。
 *
 * 前置：
 * 1. `pnpm --filter @mode/web exec vite --port 5199 --strictPort`（页面与 HMR）；
 * 2. 本机安装 Chrome（playwright-core 用 `channel: "chrome"` 启动）；
 * 3. 8787/8788 端口空闲。
 *
 * 运行：`npx tsx scripts/acceptance/remote-v3-web.mts`
 * 常驻（供人工点选）：`npx tsx scripts/acceptance/remote-v3-web.mts --serve`
 * 截图落在 `.plan/local-accept/`（本地工作区，不入库）。
 * 可选 `ACCEPT_PADDING=<bytes>` 调工作区大响应大小（默认 200000）。
 */
import { createServer } from "node:http";
import { mkdir } from "node:fs/promises";
import { chromium, type Browser, type Page } from "playwright-core";

import { createHttpServer } from "../../packages/server/src/http.js";
import { ProxyChannel, type IChannelServer } from "../../packages/rpc/src/index.js";
import { ISystemService, type ServiceCollection } from "../../packages/services/src/index.js";
import { startRemoteServer } from "../../packages/remote-server/src/index.js";
import { createRemoteV3HostService } from "../../packages/desktop/src/host/remoteV3/service.js";

const WEB_URL = "http://localhost:5199/v3";
const SHOT_DIR = process.env.ACCEPT_SHOT_DIR ?? ".plan/local-accept";
const results: Array<{ step: string; status: "pass" | "fail" | "info"; detail?: string }> = [];

function record(step: string, status: "pass" | "fail" | "info", detail?: unknown): void {
  results.push({ step, status, ...(detail === undefined ? {} : { detail: String(detail) }) });
  console.log(
    `[accept] ${status.toUpperCase()} ${step}${detail === undefined ? "" : `：${detail}`}`,
  );
}

function fakeCredentials() {
  const store = new Map<string, string>();
  return {
    store,
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  };
}

async function startFakeGithub(): Promise<{ port: number; close(): Promise<void> }> {
  const server = createServer((req, res) => {
    const send = (body: unknown) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (req.url === "/login/device/code") {
      send({
        device_code: "local-device-code",
        user_code: "LOCAL-1234",
        verification_uri: `http://127.0.0.1:${server.address()?.port ?? 0}/login/device`,
        expires_in: 900,
        interval: 1,
      });
      return;
    }
    if (req.url === "/login/oauth/access_token") {
      send({ access_token: "local-github-token", token_type: "bearer", scope: "read:user" });
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  };
}

interface Stack {
  url: string;
  setOnline(online: boolean): Promise<void>;
  close(): Promise<void>;
}

const WORKSPACE_TOKEN = "local-workspace-token";

/** 最小「Host 本机 LAN 服务」：真实 HTTP/WS + systemService.info，供工作区隧道验收。 */
async function startWorkspaceApi(): Promise<{
  port: number;
  close(): Promise<void>;
}> {
  const services = {
    getOptional: () => undefined,
    exposeOnChannelServer: (server: IChannelServer) => {
      server.registerChannel(
        ISystemService.channelName,
        ProxyChannel.fromService({
          info: async () => ({
            homedir: ".plan/local-accept/workspace",
            platform: process.platform,
            // 大响应：验证分片/加密分片/DataChannel 分片整条链路不丢字节。
            padding: `${"x".repeat(Number(process.env.ACCEPT_PADDING ?? 200_000))}END-OF-LARGE`,
          }),
        }),
      );
    },
  } as unknown as ServiceCollection;
  const server = createHttpServer(services, 0, {
    authToken: WORKSPACE_TOKEN,
    verifyAuthToken: (presented) => presented === WORKSPACE_TOKEN,
    serverId: "local-accept",
    name: "本地验收工作区",
  });
  if (!server.listening) {
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  }
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  };
}

async function startStack(input: {
  port: number;
  deviceName: string;
  providerUserId: string;
  githubPort: number;
  workspacePort?: number;
}): Promise<Stack> {
  const server = await startRemoteServer({
    port: input.port,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: `local-acceptance-secret-${input.port}-xx`,
    githubClientId: "local-accept-client",
    githubDeviceBaseUrl: `http://127.0.0.1:${input.githubPort}`,
    githubVerifier: async () => ({
      providerUserId: input.providerUserId,
      login: input.providerUserId,
      name: `${input.deviceName} 账号`,
    }),
    allowedOrigins: ["http://localhost:5199", "http://127.0.0.1:5199"],
    log: (line) => console.log(`[server:${input.port}] ${line}`),
  });
  const host = await createRemoteV3HostService({
    credentials: fakeCredentials(),
    env: {},
    log: (line) => console.log(`[host:${input.port}] ${line}`),
    version: "0.0.1-accept",
    deviceName: input.deviceName,
    platform: "desktop",
    listWorkspaces: async () => [{ path: "/tmp/demo", name: "demo" }],
    getLocalApi: async () =>
      input.workspacePort
        ? { baseUrl: `http://127.0.0.1:${input.workspacePort}`, token: WORKSPACE_TOKEN }
        : null,
  });
  const login = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "local-github-token" }),
  });
  const { sessionToken } = (await login.json()) as { sessionToken: string };
  const state = await host.configure({
    serverUrl: server.url,
    webSessionToken: sessionToken,
    deviceName: input.deviceName,
  });
  record(
    `Host 上线（${input.deviceName}）`,
    state.status === "online" ? "pass" : "fail",
    state.error ?? state.deviceId,
  );
  return {
    url: server.url,
    async setOnline(online: boolean) {
      if (!online) {
        await host.disconnectAccount();
        return;
      }
      const response = await fetch(`${server.url}/api/v1/auth/github`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ accessToken: "local-github-token" }),
      });
      const { sessionToken: fresh } = (await response.json()) as { sessionToken: string };
      const restored = await host.configure({
        serverUrl: server.url,
        webSessionToken: fresh,
        deviceName: input.deviceName,
      });
      if (restored.status !== "online") {
        throw new Error(`Host 恢复失败：${restored.error ?? restored.status}`);
      }
    },
    async close() {
      await host.disconnectAccount();
      await server.close();
    },
  };
}

async function login(page: Page, serverUrl: string): Promise<void> {
  await page.locator("#v3-server-url").fill(serverUrl);
  await page.getByRole("button", { name: "使用 GitHub 登录" }).click();
  await page.getByRole("button", { name: "退出" }).waitFor({ timeout: 25_000 });
  await page.getByText("本机 Web 设备").waitFor();
}

async function rpcText(page: Page, hostName: string): Promise<string> {
  const row = page.locator("li", { hasText: hostName }).first();
  await row.getByRole("button", { name: "加密 RPC" }).click();
  await page.waitForFunction(
    () => {
      const pre = document.querySelector("pre");
      return Boolean(pre && !pre.textContent?.includes("正在建立"));
    },
    { timeout: 30_000 },
  );
  return await page.locator("pre").innerText();
}

async function main(): Promise<void> {
  await mkdir(SHOT_DIR, { recursive: true });
  const github = await startFakeGithub();
  const workspaceApi = await startWorkspaceApi();
  const stackA = await startStack({
    port: 8787,
    deviceName: "本地验收主机",
    providerUserId: "local-user-a",
    githubPort: github.port,
    workspacePort: workspaceApi.port,
  });
  const stackB = await startStack({
    port: 8788,
    deviceName: "第二台主机",
    providerUserId: "local-user-b",
    githubPort: github.port,
  });
  if (process.argv.includes("--serve")) {
    console.log(`[accept] SERVE A=${stackA.url} B=${stackB.url} web=${WEB_URL}`);
    await new Promise(() => undefined);
  }
  const browser: Browser = await chromium.launch({
    channel: "chrome",
    headless: true,
    args: ["--allow-loopback-in-peer-connection", "--disable-features=WebRtcHideLocalIpsWithMdns"],
  });
  const pageErrors: string[] = [];
  const watch = (page: Page): void => {
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") pageErrors.push(message.text());
    });
  };

  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 960 } });
    const page = await context.newPage();
    watch(page);
    await page.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    await page.getByRole("heading", { name: "远程控制 · Web" }).waitFor();

    const initialText = await page.locator("body").innerText();
    record(
      "未登录不展示已登录区块",
      initialText.includes("使用 GitHub 登录") && !initialText.includes("本机 Web 设备")
        ? "pass"
        : "fail",
    );
    await page.locator("#v3-server-url").fill(stackA.url);
    record(
      "输入服务地址后仍保留登录入口",
      (await page.locator("body").innerText()).includes("使用 GitHub 登录") ? "pass" : "fail",
    );

    await login(page, stackA.url);
    record("登录账号 A（Device Flow）", "pass");
    await page.getByText("本地验收主机").waitFor({ timeout: 15_000 });
    record("账号 A 目录发现 Host", "pass");

    const rpcP2P = await rpcText(page, "本地验收主机");
    record(
      "账号 A 加密 RPC 选路",
      rpcP2P.includes("传输：p2p") ? "pass" : rpcP2P.includes("传输：relay") ? "fail" : "info",
      rpcP2P.split("\n")[0],
    );
    record("remote.host.info 返回设备信息", rpcP2P.includes("本地验收主机") ? "pass" : "fail");
    await page.screenshot({ path: `${SHOT_DIR}/01-rpc-p2p.png`, fullPage: true });

    const row = page.locator("li", { hasText: "本地验收主机" }).first();
    await row.getByRole("button", { name: "工作区隧道" }).click();
    try {
      await page.waitForFunction(
        () => {
          const text = document.body.textContent ?? "";
          return (
            text.includes("local-accept/workspace") || text.includes("HOST_LOCAL_API_UNAVAILABLE")
          );
        },
        { timeout: 25_000 },
      );
    } catch (error) {
      const dump = (await page.locator("body").innerText()).slice(0, 1_200);
      console.log(`[debug] workspace body=${dump.replace(/\s+/g, " ")}`);
      throw error;
    }
    const workspaceBody = await page.locator("body").innerText();
    const workspaceOk = workspaceBody.includes("local-accept/workspace");
    const largeOk = workspaceBody.includes("END-OF-LARGE");
    record(
      "工作区隧道（真实本机 LAN 服务）",
      workspaceOk ? "pass" : "info",
      workspaceOk
        ? "systemService.info 经 /ws/host 隧道返回"
        : workspaceBody.split("\n").find((line) => line.includes("HOST_LOCAL_API_UNAVAILABLE")),
    );
    record(
      `工作区大响应（${process.env.ACCEPT_PADDING ?? "200k"}B）完整返回`,
      largeOk ? "pass" : "fail",
      largeOk ? "尾部标记 END-OF-LARGE 可见" : "响应被截断",
    );
    await page.screenshot({ path: `${SHOT_DIR}/02-workspace.png`, fullPage: true });

    // 远程桌面：轻量 Host 没有桌面服务，必须给出可返回的错误态而不是白屏/卡死。
    await row.getByRole("button", { name: "进入桌面" }).click();
    const backToDevices = page.getByRole("button", { name: /返回设备|Back to devices/ }).last();
    await backToDevices.waitFor({ timeout: 30_000 });
    record("远程桌面缺少 Host 服务时优雅退出", "pass");
    await page.screenshot({ path: `${SHOT_DIR}/02b-desktop-error.png`, fullPage: true });
    await backToDevices.click();
    await page.getByText("本地验收主机").waitFor({ timeout: 10_000 });
    record("远程桌面错误后返回设备列表", "pass");

    // Host 下线/恢复：目录状态与按钮禁用必须跟随刷新。
    await stackA.setOnline(false);
    await page.getByRole("button", { name: "刷新设备" }).click();
    const hostRow = page.locator("li", { hasText: "本地验收主机" }).first();
    await hostRow.getByText("离线").waitFor({ timeout: 15_000 });
    record("Host 下线后目录显示离线", "pass");
    record(
      "Host 离线时操作按钮禁用",
      (await hostRow.getByRole("button", { name: "加密 RPC" }).isDisabled()) ? "pass" : "fail",
    );
    await page.screenshot({ path: `${SHOT_DIR}/02c-host-offline.png`, fullPage: true });

    await stackA.setOnline(true);
    await page.getByRole("button", { name: "刷新设备" }).click();
    await hostRow.getByText("在线").waitFor({ timeout: 20_000 });
    record("Host 恢复后目录回到在线", "pass");

    await page.getByRole("button", { name: "退出" }).click();
    await page.getByText("已保存的账号").waitFor({ timeout: 10_000 });
    record("退出账号 A 后仍保留记录", "pass");
    record(
      "退出后清空旧错误提示",
      (await page.locator("body").innerText()).includes("HOST_LOCAL_API_UNAVAILABLE")
        ? "fail"
        : "pass",
    );

    await login(page, stackB.url);
    await page.getByText("第二台主机").waitFor({ timeout: 15_000 });
    record("登录账号 B 并发现独立 Host", "pass");
    const rpcB = await rpcText(page, "第二台主机");
    record(
      "账号 B 加密 RPC 选路",
      rpcB.includes("传输：p2p") ? "pass" : "info",
      rpcB.split("\n")[0],
    );
    await page.getByRole("button", { name: "退出" }).click();
    await page.getByText("已保存的账号").waitFor({ timeout: 10_000 });
    const accountsText = await page.locator("body").innerText();
    record(
      "两个账号同时保留",
      accountsText.includes(stackA.url) && accountsText.includes(stackB.url) ? "pass" : "fail",
    );
    await page.screenshot({ path: `${SHOT_DIR}/04-two-accounts.png`, fullPage: true });

    const rowA = page.locator("li", { hasText: stackA.url }).first();
    await rowA.getByRole("button", { name: "使用", exact: true }).click();
    await login(page, stackA.url);
    await page.getByText("本地验收主机").waitFor({ timeout: 15_000 });
    record(
      "切回账号 A：目录只见 A 的 Host",
      (await page.locator("body").innerText()).includes("第二台主机") ? "fail" : "pass",
    );
    await page.screenshot({ path: `${SHOT_DIR}/05-switch-back.png`, fullPage: true });

    await page.getByRole("button", { name: "退出" }).click();
    await page.getByText("已保存的账号").waitFor({ timeout: 10_000 });
    await page
      .locator("li", { hasText: stackA.url })
      .first()
      .getByRole("button", { name: "删除", exact: true })
      .click();
    await page
      .locator("li", { hasText: stackA.url })
      .first()
      .waitFor({ state: "detached", timeout: 10_000 });
    record(
      "删除账号 A 不影响账号 B",
      (await page.locator("body").innerText()).includes(stackB.url) ? "pass" : "fail",
    );
    await page.screenshot({ path: `${SHOT_DIR}/06-delete-a.png`, fullPage: true });

    // Relay 路径：新上下文禁用浏览器 WebRTC，验证自动回落且业务可用。
    const relayContext = await browser.newContext({ viewport: { width: 1280, height: 960 } });
    await relayContext.addInitScript(() => {
      Object.defineProperty(window, "RTCPeerConnection", { value: undefined, configurable: true });
    });
    const relayPage = await relayContext.newPage();
    watch(relayPage);
    await relayPage.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    await login(relayPage, stackA.url);
    await relayPage.getByText("本地验收主机").waitFor({ timeout: 15_000 });
    const relayRpc = await rpcText(relayPage, "本地验收主机");
    record(
      "禁用 WebRTC 后回落 Relay 且 RPC 成功",
      relayRpc.includes("传输：relay") && relayRpc.includes("本地验收主机") ? "pass" : "fail",
      relayRpc.split("\n")[0],
    );
    await relayPage.screenshot({ path: `${SHOT_DIR}/07-relay.png`, fullPage: true });
    await relayContext.close();

    // 手机宽度：/v3 不能出现横向滚动，核心操作仍可用。
    const mobileContext = await browser.newContext({
      viewport: { width: 390, height: 844 },
      deviceScaleFactor: 2,
    });
    const mobilePage = await mobileContext.newPage();
    watch(mobilePage);
    await mobilePage.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    await login(mobilePage, stackA.url);
    await mobilePage.getByText("本地验收主机").waitFor({ timeout: 15_000 });
    const overflow = await mobilePage.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    );
    record("手机宽度无横向滚动", overflow <= 1 ? "pass" : "fail", `溢出 ${overflow}px`);
    const mobileRpc = await rpcText(mobilePage, "本地验收主机");
    record(
      "手机宽度下加密 RPC 可用",
      mobileRpc.includes("传输：p2p") ? "pass" : "fail",
      mobileRpc.split("\n")[0],
    );
    await mobilePage.screenshot({ path: `${SHOT_DIR}/08-mobile.png`, fullPage: true });
    await mobileContext.close();

    // 不可达服务地址：必须给出错误，而不是停在加载态。
    const negativeContext = await browser.newContext({ viewport: { width: 1280, height: 960 } });
    const negativePage = await negativeContext.newPage();
    watch(negativePage);
    await negativePage.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    await negativePage.locator("#v3-server-url").fill("http://127.0.0.1:59999");
    await negativePage.getByRole("button", { name: "使用 GitHub 登录" }).click();
    await negativePage.waitForFunction(
      () =>
        [...document.querySelectorAll("p")].some(
          (element) =>
            element.className.includes("destructive") && (element.textContent ?? "").length > 0,
        ),
      { timeout: 15_000 },
    );
    const loginError =
      (await negativePage.locator("p.destructive, p[class*='destructive']").first().innerText()) ||
      "";
    record("不可达服务地址显示错误", loginError.length > 0 ? "pass" : "fail", loginError);
    await negativePage.screenshot({ path: `${SHOT_DIR}/09-invalid-url.png`, fullPage: true });
    await negativeContext.close();
  } finally {
    const failed = results.filter((entry) => entry.status === "fail").length;
    if (pageErrors.length)
      console.log(`[accept] pageErrors=${JSON.stringify(pageErrors.slice(0, 8))}`);
    console.log(`[accept] results=${JSON.stringify(results)}`);
    console.log(
      `[accept] summary pass=${results.filter((r) => r.status === "pass").length} info=${results.filter((r) => r.status === "info").length} fail=${failed}`,
    );
    await browser.close();
    await stackA.close();
    await stackB.close();
    await workspaceApi.close();
    await github.close();
  }
}

main().catch((error) => {
  console.error("[accept] 失败", error);
  process.exitCode = 1;
});
