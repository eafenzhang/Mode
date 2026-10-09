import test from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BrokerError, BROKER_SOCKET_ENV, CuaHelperError, isCuaHelperError } from "../broker.js";
import {
  buildHelperOpenArgs,
  clearCuaProductHelperAgentEnvUnavailable,
  createCuaProductMcpServerResolver,
  createProductCuaHelperHost,
  CuaProductHelperWorkspaceRegistry,
  cuaBrokerRefreshMarkerPath,
  hasCuaProductHelperAgentEnvUnavailable,
  isCuaLocalDevelopmentRuntime,
  isOfficialCuaPluginEnabledForWorkspace,
  isPotentialModeCuaAgentMcpServer,
  isScreenCaptureProbeSuccess,
  markCuaProductHelperAgentEnvUnavailable,
  publishCuaBrokerRefreshMarker,
  reapOrphanedHelpers,
  waitForCuaHelperStartup,
} from "../broker-server.js";

// 与 packages/shared/src/runtimeEnv.ts:128 的 MODE_CUA_PLUGIN_AUTHORITY_ENV_KEY 同值；
// mode-cua 零依赖不能引 shared，按消费方 env key 字面实现（报告注明单一事实源来源行）。
const PLUGIN_AUTHORITY_ENV = "MODE_CUA_PLUGIN_AUTHORITY";
const OFFICIAL_CUA_PLUGIN_ID = "computer-use@mode-plugins-official";
// resolver 的 win32 一期门控读 process.platform；平台分支用例在非 win32 上跳过（本仓开发机为 win32）。
const isWin32 = process.platform === "win32";

const tempDirs = [];
function makeTempHome() {
  const dir = mkdtempSync(join(tmpdir(), "cua-resolver-test-"));
  tempDirs.push(dir);
  return dir;
}
test.after(() => {
  for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** 写一份最小 ~/.mode/cli/config.json 到临时 HOME，返回恢复 process.env.HOME 的函数。 */
function writeCliConfig(config) {
  const home = makeTempHome();
  const configDir = join(home, ".mode", "cli");
  mkdirSync(configDir, { recursive: true });
  writeFileSync(join(configDir, "config.json"), JSON.stringify(config), "utf8");
  const previous = process.env.HOME;
  process.env.HOME = home;
  return () => {
    if (previous === undefined) delete process.env.HOME;
    else process.env.HOME = previous;
  };
}

function makeHost(options = {}) {
  const state = { startCalls: 0, restartCalls: 0, failStart: options.failStart === true };
  const host = {
    running: false,
    socketPath: null,
    pluginAuthority: null,
    async start() {
      state.startCalls += 1;
      if (state.failStart) throw new Error("helper launch failed");
      // WindowsCuaHelperHost 的 start() 返回 {socketPath, pluginAuthority}（见
      // services/src/cua-permission-broker/windowsCuaDevHelperHost.ts handle 构造）。
      return { socketPath: "\\\\.\\pipe\\mode-cua-fake", pluginAuthority: "authority-fake" };
    },
    async restart() {
      state.restartCalls += 1;
      return { socketPath: "\\\\.\\pipe\\mode-cua-fake", pluginAuthority: "authority-fake" };
    },
  };
  return { host, state };
}

function envEntry(env, name) {
  assert.ok(Array.isArray(env), "expected mode-cua server env array");
  return env.find((entry) => entry.name === name);
}

function cuaCandidateServer() {
  return { name: "computer-use", command: "uvx", args: ["mode-cua"], env: [] };
}

test(
  "win32: 插件启用 → resolveMcpServers 拉起 host 并把凭据注入 mode-cua server env",
  { skip: !isWin32 },
  async () => {
    const restoreHome = writeCliConfig({
      plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } },
    });
    try {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, {});
      const other = { name: "other", command: "npx", args: ["x"], env: [] };
      const servers = [cuaCandidateServer(), other];
      const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
      assert.equal(state.startCalls, 1, "win32 + 插件启用必须 await host.start()");
      assert.notEqual(resolved, servers, "注入凭据后应返回新数组");
      assert.equal(resolved[1], other, "非候选 server 保持同一引用");
      const socket = envEntry(resolved[0].env, BROKER_SOCKET_ENV);
      const authority = envEntry(resolved[0].env, PLUGIN_AUTHORITY_ENV);
      assert.deepEqual(socket, { name: BROKER_SOCKET_ENV, value: "\\\\.\\pipe\\mode-cua-fake" });
      assert.deepEqual(authority, { name: PLUGIN_AUTHORITY_ENV, value: "authority-fake" });
    } finally {
      restoreHome();
    }
  },
);

test("win32: 插件未启用 → 不 start，servers 原样返回（同一引用）", { skip: !isWin32 }, async () => {
  const restoreHome = writeCliConfig({ plugins: { enabledPlugins: {} } });
  try {
    const { host, state } = makeHost();
    const resolver = createCuaProductMcpServerResolver(host, {});
    const servers = [cuaCandidateServer()];
    const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
    assert.equal(state.startCalls, 0, "插件未启用绝不 start");
    assert.equal(resolved, servers);
  } finally {
    restoreHome();
  }
});

test(
  "win32: 插件启用但 servers 无候选 → 仍 start（demand boundary），返回原引用",
  { skip: !isWin32 },
  async () => {
    const restoreHome = writeCliConfig({
      plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } },
    });
    try {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, {});
      const servers = [{ name: "other", command: "npx", args: ["x"], env: [] }];
      const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
      assert.equal(state.startCalls, 1);
      assert.equal(resolved, servers, "没有候选时不改数组，消费方跳过参数重建");
    } finally {
      restoreHome();
    }
  },
);

test(
  "win32: 活跃 turn → 不 start（避免 turn 中途拉起/轮换 Helper）",
  { skip: !isWin32 },
  async () => {
    const restoreHome = writeCliConfig({
      plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } },
    });
    try {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, { hasActiveTurn: () => true });
      const servers = [cuaCandidateServer()];
      const resolved = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(state.startCalls, 0);
      assert.equal(resolved, servers);
    } finally {
      restoreHome();
    }
  },
);

test(
  "win32: start 失败 → 不抛、原样返回、标 unavailable；后续短路；clear 后恢复",
  { skip: !isWin32 },
  async () => {
    const restoreHome = writeCliConfig({
      plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } },
    });
    try {
      const { host, state } = makeHost({ failStart: true });
      const resolver = createCuaProductMcpServerResolver(host, {});
      const servers = [cuaCandidateServer()];

      const first = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(first, servers, "失败必须原样返回");
      assert.equal(state.startCalls, 1);
      assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), true, "start 失败要落标记");

      // 失败后即使 host 已可用，也必须短路直返（直到显式 clear）。
      state.failStart = false;
      const second = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(second, servers);
      assert.equal(state.startCalls, 1, "unavailable 期间不得再 start（短路）");

      clearCuaProductHelperAgentEnvUnavailable(host);
      assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
      const third = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(state.startCalls, 2, "clear 后恢复拉起");
      assert.equal(envEntry(third[0].env, BROKER_SOCKET_ENV)?.value, "\\\\.\\pipe\\mode-cua-fake");
    } finally {
      restoreHome();
    }
  },
);

test(
  "win32: restart 委托 host.restart；restartAfterPermissionGrant 直接返回不抛、不碰 host",
  { skip: !isWin32 },
  async () => {
    const { host, state } = makeHost();
    const resolver = createCuaProductMcpServerResolver(host, {});
    await resolver.restart();
    assert.equal(state.restartCalls, 1, "restart 必须委托 host.restart()");
    // Windows 无 TCC 授权流：授权后 restart 是 no-op（brief 一期语义），绝不抛错。
    await resolver.restartAfterPermissionGrant("onboarding-session");
    assert.equal(state.restartCalls, 1, "win32 的 restartAfterPermissionGrant 不触发重启");
  },
);

test("waitForCuaHelperStartup: 成功/失败透传；deadline 真超时拒绝 broker_unavailable", async () => {
  assert.equal(await waitForCuaHelperStartup(Promise.resolve(42), 1000), 42);
  const boom = new Error("launch failed");
  await assert.rejects(
    waitForCuaHelperStartup(Promise.reject(boom), 1000),
    (error) => error === boom,
  );
  // 无 deadline → 恒等透传（d.ts deadlineMs? 为可选参）。
  assert.equal(await waitForCuaHelperStartup(Promise.resolve("late")), "late");

  // 超时：内部 promise 300ms 后才 settle，20ms deadline 必须先拒绝。
  const slow = new Promise((resolve) => setTimeout(() => resolve("too late"), 300));
  await assert.rejects(
    waitForCuaHelperStartup(slow, 20),
    (error) => error instanceof BrokerError && error.code === "broker_unavailable",
    "deadline 到期应拒绝 BrokerError{code:'broker_unavailable'}",
  );
});

test("waitForCuaHelperStartup: 超时后晚到的 settle 不产生未处理拒绝", async () => {
  let rejectLate;
  const pending = new Promise((resolve, reject) => {
    rejectLate = reject;
    // 兜底 settle：RED（恒等透传）下断言在 250ms 失败而不是挂死；GREEN 下 deadline 先拒绝，
    // 兜底 resolve 成为 no-op。
    setTimeout(resolve, 250);
  });
  await assert.rejects(
    waitForCuaHelperStartup(pending, 10),
    (error) => error instanceof BrokerError && error.code === "broker_unavailable",
  );
  rejectLate(new Error("late failure")); // 已挂 handlers，不应触发 unhandledRejection
  await new Promise((resolve) => setTimeout(resolve, 20));
});

test("isOfficialCuaPluginEnabledForWorkspace 读 ~/.mode/cli/config.json 的 enabledPlugins（旧 id 归一）", async () => {
  const enabledHome = makeTempHome();
  const enabledDir = join(enabledHome, ".mode", "cli");
  mkdirSync(enabledDir, { recursive: true });
  // 存量写法：改名前的 mode-cua 名 + 旧市场段；读时必须归一到当前 id。
  writeFileSync(
    join(enabledDir, "config.json"),
    JSON.stringify({ plugins: { enabledPlugins: { "mode-cua@zcode-plugins-official": true } } }),
    "utf8",
  );
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env: { HOME: enabledHome } }), true);

  const home = makeTempHome();
  const configDir = join(home, ".mode", "cli");
  mkdirSync(configDir, { recursive: true });
  const configPath = join(configDir, "config.json");
  const env = { HOME: home };

  // 缺省（computer-use 默认关闭，shared/plugin-marketplaces.ts 注释）→ false。
  writeFileSync(configPath, JSON.stringify({ plugins: { enabledPlugins: {} } }), "utf8");
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 显式 false → false。
  writeFileSync(
    configPath,
    JSON.stringify({ plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: false } } }),
    "utf8",
  );
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 显式 true → true。
  writeFileSync(
    configPath,
    JSON.stringify({ plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } } }),
    "utf8",
  );
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env, workingDirectory: "C:\\ws" }), true);
  // plugins.enabled 总开关关闭 → false（fail-closed）。
  writeFileSync(
    configPath,
    JSON.stringify({
      plugins: { enabled: false, enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } },
    }),
    "utf8",
  );
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // suppressedBuiltins（卸载抑制）优先 → false。
  writeFileSync(
    configPath,
    JSON.stringify({
      plugins: {
        enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true },
        suppressedBuiltins: [OFFICIAL_CUA_PLUGIN_ID],
      },
    }),
    "utf8",
  );
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 配置文件不存在 / 不可读 → false。（HOME 缺省会落到 homedir() 读真机配置，不在此断言具体值。）
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env: { HOME: makeTempHome() } }), false);
  assert.equal(typeof isOfficialCuaPluginEnabledForWorkspace({}), "boolean");
});

test("isPotentialModeCuaAgentMcpServer 识别 mode-cua 形态候选（与 shared/mcp.ts 单一事实源同判定）", async () => {
  // stdio：包规格在 command / args（含点号子模块、git、本地路径形态）。
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "computer-use",
      command: "uvx",
      args: ["mode-cua"],
      env: [],
    }),
    true,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({ name: "custom", command: "mode-cua", args: [], env: [] }),
    true,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "custom",
      command: "python",
      args: ["-m", "mode_cua.server"],
      env: [],
    }),
    true,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "custom",
      command: "uvx",
      args: ["mode-cua@1.2.3"],
      env: [],
    }),
    true,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "custom",
      command: "uvx",
      args: ["C:\\proj\\mode-cua"],
      env: [],
    }),
    true,
  );
  // server name 即 computer-use。
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "computer-use",
      command: "node",
      args: ["x.js"],
      env: [],
    }),
    true,
  );
  // official plugin id 标记（ModeAgentMcpServer 的 env 数组形态 + McpServerConfig 的 env record 形态）。
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "node_repl",
      command: "node",
      args: [],
      env: [{ name: "MODE_PLUGIN_ID", value: OFFICIAL_CUA_PLUGIN_ID }],
    }),
    true,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "node_repl",
      command: "node",
      args: [],
      env: { MODE_PLUGIN_ID: OFFICIAL_CUA_PLUGIN_ID },
    }),
    true,
  );
  // 非候选：无关 stdio / http / 非法输入。
  assert.equal(
    isPotentialModeCuaAgentMcpServer({
      name: "fs",
      command: "npx",
      args: ["@modelcontextprotocol/fs"],
      env: [],
    }),
    false,
  );
  assert.equal(
    isPotentialModeCuaAgentMcpServer({ name: "remote", type: "http", url: "https://x" }),
    false,
  );
  assert.equal(isPotentialModeCuaAgentMcpServer(undefined), false);
  assert.equal(isPotentialModeCuaAgentMcpServer("computer-use"), false);
});

test("keep-stub 恒值钉死（每个保留 stub 引用消费方裁决理由）", async () => {
  // createProductCuaHelperHost：仅 services node.ts:1043 darwin 分支消费；mac 一期 fail-closed，
  // 返回 unavailable host（值不变）。
  const macHost = createProductCuaHelperHost({});
  assert.equal(macHost.running, false);
  assert.equal(macHost.socketPath, null);
  assert.equal(macHost.pluginAuthority, null);
  await assert.rejects(
    macHost.start(),
    (error) => isCuaHelperError(error) && error.message.includes("not available in this build"),
  );

  // isScreenCaptureProbeSuccess：唯一消费方 runCuaScreenCaptureReadinessProbe（node.ts:795）
  // 只被 darwin 门控的 getStatus（node.ts:1914 shouldUseCuaPermissionService）调用 → mac-only 保持 stub。
  assert.equal(isScreenCaptureProbeSuccess({ ok: true }), false);
  assert.equal(isScreenCaptureProbeSuccess({ ok: false }), false);
  assert.equal(isScreenCaptureProbeSuccess(undefined), false);

  // reapOrphanedHelpers：唯一消费方 node.ts:1028 `platform === "darwin"` → mac-only 保持 stub。
  assert.equal(await reapOrphanedHelpers({}), undefined);

  // refresh-marker 对：仓库内零调用方；marker 链休眠（buildCuaProductHelperAgentEnv 从不下发
  // MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER，createComputerUseRuntime 也不消费 refreshMarkerPath
  // —— index.js:1270 注释自证）→ 保持 stub。
  assert.equal(cuaBrokerRefreshMarkerPath("\\\\.\\pipe\\mode-cua-x"), undefined);
  assert.deepEqual(await publishCuaBrokerRefreshMarker("\\\\.\\pipe\\mode-cua-x"), {
    path: undefined,
  });

  // buildHelperOpenArgs / isCuaLocalDevelopmentRuntime：仅 node.ts:1845/1849 的
  // launchStandaloneCuaHelperForStatus（darwin getStatus 链）消费 → mac-only 保持 stub（无 dev argv）。
  assert.deepEqual(buildHelperOpenArgs({ spec: {} }, 123), []);
  assert.equal(isCuaLocalDevelopmentRuntime({}, false), false);

  // CuaProductHelperWorkspaceRegistry：setEnabled 只写不读（node.ts:2194/2206/2217 三处写、无读方）
  // → 保持 no-op stub（值不变）。
  const registry = new CuaProductHelperWorkspaceRegistry();
  assert.equal(registry.setEnabled({ workspacePath: "C:\\ws" }, true), undefined);
});

test("mark / has / clear unavailable：进程内标志位按 host 对象隔离", async () => {
  const { host } = makeHost();
  const other = makeHost().host;
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
  markCuaProductHelperAgentEnvUnavailable(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), true);
  // 按 host 键隔离：另一个 host 不受影响。
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(other), false);
  clearCuaProductHelperAgentEnvUnavailable(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
  // 幂等：重复 mark/clear 不抛。
  markCuaProductHelperAgentEnvUnavailable(host);
  markCuaProductHelperAgentEnvUnavailable(host);
  clearCuaProductHelperAgentEnvUnavailable(host);
  clearCuaProductHelperAgentEnvUnavailable(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
});

test("CuaHelperError 仍是 broker-server 现有失败面（wait 超时用 BrokerError，不改错误类语义）", async () => {
  const error = new CuaHelperError("boom", { code: "caller_timeout" });
  assert.equal(isCuaHelperError(error), true);
  assert.equal(error.code, "caller_timeout");
});
