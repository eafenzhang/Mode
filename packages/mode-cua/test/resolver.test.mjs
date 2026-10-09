import test from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { BrokerError, BROKER_SOCKET_ENV, isCuaHelperError } from "../broker.js";
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
// resolver 的 win32 一期门控读 process.platform；平台分支用例在非 win32 上 skip（本仓开发机为 win32）。
const isWin32 = process.platform === "win32";
const FAKE_SOCKET = "\\\\.\\pipe\\mode-cua-fake";
const FAKE_AUTHORITY = "authority-fake";
// 便捷夹具：user 层「官方 CUA 插件启用」的 config.json 内容。
const CUA_ON = { plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true } } };

const tempDirs = [];
function makeTempHome() {
  const dir = mkdtempSync(join(tmpdir(), "cua-resolver-test-"));
  tempDirs.push(dir);
  return dir;
}
test.after(() => {
  for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** 写一份配置文件（mode.json 或 .mode/config.json 均可），自动建父目录。 */
function writeProjectConfigFile(path, config) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(config), "utf8");
}

/** 只写 computer-use enabledPlugins 一个键的配置。 */
function writeEnabledPluginsFile(path, enabled) {
  writeProjectConfigFile(path, {
    plugins: { enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: enabled } },
  });
}

/** 在临时 HOME（写入 ~/.mode/cli/config.json）下执行，finally 恢复 process.env.HOME。 */
async function withHome(config, run) {
  const home = makeTempHome();
  writeProjectConfigFile(join(home, ".mode", "cli", "config.json"), config);
  const previous = process.env.HOME;
  process.env.HOME = home;
  try {
    return await run();
  } finally {
    if (previous === undefined) delete process.env.HOME;
    else process.env.HOME = previous;
  }
}

/** 临时覆写 process.platform（属性 descriptor 可配置），finally 恢复原状。 */
async function withPlatform(value, run) {
  const descriptor = Object.getOwnPropertyDescriptor(process, "platform");
  Object.defineProperty(process, "platform", { ...descriptor, value });
  try {
    return await run();
  } finally {
    Object.defineProperty(process, "platform", descriptor);
  }
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
      return { socketPath: FAKE_SOCKET, pluginAuthority: FAKE_AUTHORITY };
    },
    async restart() {
      state.restartCalls += 1;
      return { socketPath: FAKE_SOCKET, pluginAuthority: FAKE_AUTHORITY };
    },
  };
  return { host, state };
}

/** 断言 env 数组里恰有且只有 socket+authority 两条注入（ModeAgentMcpServer 形态）。 */
function assertInjected(env, socket, authority) {
  assert.ok(Array.isArray(env), "expected {name,value}[] env");
  const pairs = env.filter((e) => e.name === BROKER_SOCKET_ENV || e.name === PLUGIN_AUTHORITY_ENV);
  assert.deepEqual(pairs, [
    { name: BROKER_SOCKET_ENV, value: socket },
    { name: PLUGIN_AUTHORITY_ENV, value: authority },
  ]);
}

/** stdio server 快速构造（isPotential 判定用），extra 覆盖 command/args/env。 */
function stdioServer(name, extra = {}) {
  return { name, command: "uvx", args: ["mode-cua"], env: [], ...extra };
}

test(
  "win32: 插件启用 → resolveMcpServers 拉起 host 并把凭据注入 mode-cua server env",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, {});
      const other = { name: "other", command: "npx", args: ["x"], env: [] };
      const servers = [stdioServer("computer-use"), other];
      const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
      assert.equal(state.startCalls, 1, "win32 + 插件启用必须 await host.start()");
      assert.notEqual(resolved, servers, "注入凭据后应返回新数组");
      assert.equal(resolved[1], other, "非候选 server 保持同一引用");
      assertInjected(resolved[0].env, FAKE_SOCKET, FAKE_AUTHORITY);
    });
  },
);

test("win32: 插件未启用 → 不 start，servers 原样返回（同一引用）", { skip: !isWin32 }, async () => {
  await withHome({ plugins: { enabledPlugins: {} } }, async () => {
    const { host, state } = makeHost();
    const resolver = createCuaProductMcpServerResolver(host, {});
    const servers = [stdioServer("computer-use")];
    const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
    assert.equal(state.startCalls, 0, "插件未启用绝不 start");
    assert.equal(resolved, servers);
  });
});

test(
  "win32: 插件启用但 servers 无候选 → 仍 start（demand boundary），返回原引用",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, {});
      const servers = [{ name: "other", command: "npx", args: ["x"], env: [] }];
      const resolved = await resolver.resolveMcpServers(servers, { workspacePath: "C:\\ws" });
      assert.equal(state.startCalls, 1);
      assert.equal(resolved, servers, "没有候选时不改数组，消费方跳过参数重建");
    });
  },
);

test(
  "win32: 活跃 turn → 不 start（避免 turn 中途拉起/轮换 Helper）",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host, state } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, { hasActiveTurn: () => true });
      const servers = [stdioServer("computer-use")];
      const resolved = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(state.startCalls, 0);
      assert.equal(resolved, servers);
    });
  },
);

test(
  "win32: 活跃 turn + host 已在跑 → 直接注入现有 tuple、不调 start（M4）",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host, state } = makeHost();
      // warm host：running 且已有 tuple（模拟首启后 turn 进行中）。
      host.running = true;
      host.socketPath = "\\\\.\\pipe\\mode-cua-warm";
      host.pluginAuthority = "authority-warm";
      const resolver = createCuaProductMcpServerResolver(host, { hasActiveTurn: () => true });
      const servers = [stdioServer("computer-use")];
      const resolved = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(state.startCalls, 0, "活跃 turn 中不得调 start()（禁 mid-turn 重启）");
      assert.notEqual(resolved, servers, "warm host 应照常注入现有 tuple");
      assertInjected(resolved[0].env, "\\\\.\\pipe\\mode-cua-warm", "authority-warm");
    });
  },
);

test(
  "win32: start 失败 → 不抛、原样返回、标 unavailable；后续短路；clear 后恢复",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host, state } = makeHost({ failStart: true });
      const resolver = createCuaProductMcpServerResolver(host, {});
      const servers = [stdioServer("computer-use")];
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
      assertInjected(third[0].env, FAKE_SOCKET, FAKE_AUTHORITY);
    });
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
  const isBrokerUnavailable = (e) => e instanceof BrokerError && e.code === "broker_unavailable";
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
    isBrokerUnavailable,
    "deadline 到期应拒绝 BrokerError{code:'broker_unavailable'}",
  );
  // 超时后晚到的 settle 不产生未处理 rejection：deadline 先拒、内部 promise 后落。
  let rejectLate;
  const pending = new Promise((resolve, reject) => {
    rejectLate = reject;
    // 兜底 settle：RED（恒等透传）下断言在 250ms 失败而不是挂死；GREEN 下 deadline 先拒绝成 no-op。
    setTimeout(resolve, 250);
  });
  await assert.rejects(waitForCuaHelperStartup(pending, 10), isBrokerUnavailable);
  rejectLate(new Error("late failure")); // 已挂 handlers，不应触发 unhandledRejection
  await new Promise((resolve) => setTimeout(resolve, 20));
});

test("isOfficialCuaPluginEnabledForWorkspace 读 user 层 enabledPlugins（旧 id 归一）", async () => {
  // 存量写法：改名前的 mode-cua 名 + 旧市场段；读时必须归一到当前 id。
  const legacyHome = makeTempHome();
  const legacyOn = { plugins: { enabledPlugins: { "mode-cua@zcode-plugins-official": true } } };
  writeProjectConfigFile(join(legacyHome, ".mode", "cli", "config.json"), legacyOn);
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env: { HOME: legacyHome } }), true);

  const home = makeTempHome();
  const configPath = join(home, ".mode", "cli", "config.json");
  const env = { HOME: home };
  // 缺省（computer-use 默认关闭，shared/plugin-marketplaces.ts 注释）→ false。
  writeProjectConfigFile(configPath, { plugins: { enabledPlugins: {} } });
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 显式 false → false。
  writeEnabledPluginsFile(configPath, false);
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 显式 true → true（workingDirectory 指向不存在目录时 project 层无文件，user 层结论保留）。
  writeEnabledPluginsFile(configPath, true);
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env, workingDirectory: "C:\\ws" }), true);
  // plugins.enabled 总开关不进本门：bootstrap 唯一有效读位是 enabledPlugins ?? false
  // （plugins.ts:426/804），总开关在本仓只有 config get/set 往返（adapters/config/index.ts:138/413）
  // 无加载方消费——按总开关返回 false 就是 I1 同类「bootstrap 已加载、本门 false」假阴性。
  const masterOff = { plugins: { enabled: false, enabledPlugins: CUA_ON.plugins.enabledPlugins } };
  writeProjectConfigFile(configPath, masterOff);
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), true);
  // suppressedBuiltins（卸载抑制）优先 → false。
  writeProjectConfigFile(configPath, {
    plugins: {
      enabledPlugins: { [OFFICIAL_CUA_PLUGIN_ID]: true },
      suppressedBuiltins: [OFFICIAL_CUA_PLUGIN_ID],
    },
  });
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env }), false);
  // 配置文件不存在 → false。（HOME 缺省会落到 homedir() 读真机配置，不在此断言具体值。）
  assert.equal(isOfficialCuaPluginEnabledForWorkspace({ env: { HOME: makeTempHome() } }), false);
  assert.equal(typeof isOfficialCuaPluginEnabledForWorkspace({}), "boolean");
});

test("I1: workspace/project 层 enabledPlugins 参与判定（镜像 config-factory 发现 + config-merger 覆盖序）", async () => {
  // 夹具：user 层存在但无 computer-use 条目（默认关闭）；不传 env → 走 process.env
  //（writeCliConfig 已接管 HOME，与 resolver 生产路径同源）。
  const enabledFor = isOfficialCuaPluginEnabledForWorkspace;
  await withHome({ plugins: { enabledPlugins: {} } }, async () => {
    // ① workspace-only enabled → true（I1 失效场景：bootstrap 合并视图会加载插件，本门必须放行）。
    const wsA = makeTempHome();
    writeEnabledPluginsFile(join(wsA, ".mode", "config.json"), true);
    assert.equal(enabledFor({ workingDirectory: wsA }), true, "I1: 仅 workspace 层启用必须 true");
    // ② user 层 false + workspace 层 true → true（workspace 键覆盖 user 键）。
    const homeB = makeTempHome();
    writeEnabledPluginsFile(join(homeB, ".mode", "cli", "config.json"), false);
    const envB = { HOME: homeB };
    assert.equal(enabledFor({ env: envB, workingDirectory: wsA }), true, "ws true 盖 user false");
    // ③ workspace 显式 false + user true → false（覆盖序反向，plugins-command.ts:298 同款优先级）。
    const wsC = makeTempHome();
    mkdirSync(join(wsC, ".git"), { recursive: true });
    writeEnabledPluginsFile(join(wsC, ".mode", "config.json"), false);
    const homeC = makeTempHome();
    writeEnabledPluginsFile(join(homeC, ".mode", "cli", "config.json"), true);
    const envC = { HOME: homeC };
    assert.equal(enabledFor({ env: envC, workingDirectory: wsC }), false, "ws false 盖 user true");
    // ④ 同目录候选顺序 mode.json → .mode/config.json（后发现者覆盖，镜像 buildWorkspaceHookCandidatePaths）。
    const wsD = makeTempHome();
    writeEnabledPluginsFile(join(wsD, "mode.json"), true);
    writeEnabledPluginsFile(join(wsD, ".mode", "config.json"), false);
    assert.equal(enabledFor({ workingDirectory: wsD }), false);
    // ⑤ worktree 边界：child 自带 .git → 发现只到 child，parent 层不读（user 层默认关 → false）。
    const parentE = makeTempHome();
    writeEnabledPluginsFile(join(parentE, ".mode", "config.json"), true);
    const childE = join(parentE, "child");
    mkdirSync(join(childE, ".git"), { recursive: true });
    assert.equal(enabledFor({ workingDirectory: childE }), false);
    // ⑥ 双层全无 → false。
    assert.equal(enabledFor({ workingDirectory: makeTempHome() }), false);
  });
});

test("isPotentialModeCuaAgentMcpServer 识别 mode-cua 形态候选（与 shared/mcp.ts 单一事实源同判定）", async () => {
  const isCandidate = isPotentialModeCuaAgentMcpServer;
  // stdio：包规格在 command / args（点号子模块、git、本地路径形态）与 server name；外加 official
  // plugin id 标记（ModeAgentMcpServer env 数组 + McpServerConfig env record 双形态）。
  const pluginIdEnv = [{ name: "MODE_PLUGIN_ID", value: OFFICIAL_CUA_PLUGIN_ID }];
  const cases = [
    [stdioServer("computer-use"), true],
    [stdioServer("custom", { command: "mode-cua", args: [] }), true],
    [stdioServer("custom", { command: "python", args: ["-m", "mode_cua.server"] }), true],
    [stdioServer("custom", { args: ["mode-cua@1.2.3"] }), true],
    [stdioServer("custom", { args: ["C:\\proj\\mode-cua"] }), true],
    [stdioServer("computer-use", { command: "node", args: ["x.js"] }), true],
    [stdioServer("node_repl", { command: "node", args: [], env: pluginIdEnv }), true],
    [
      stdioServer("node_repl", {
        command: "node",
        args: [],
        env: { MODE_PLUGIN_ID: OFFICIAL_CUA_PLUGIN_ID },
      }),
      true,
    ],
    // 非候选：无关 stdio / http / 非法输入。
    [stdioServer("fs", { command: "npx", args: ["@modelcontextprotocol/fs"] }), false],
    [{ name: "remote", type: "http", url: "https://x" }, false],
    [undefined, false],
    ["computer-use", false],
  ];
  for (const [server, expected] of cases) {
    assert.equal(isCandidate(server), expected, String(server?.name ?? server));
  }
});

test(
  "win32: M5 注入覆盖 record-form env 与 env 缺失形态（upsertBrokerEnv 全分支）",
  { skip: !isWin32 },
  async () => {
    await withHome(CUA_ON, async () => {
      const { host } = makeHost();
      const resolver = createCuaProductMcpServerResolver(host, {});
      // record-form env（McpServerConfig 形态）：既有键保留，broker 键并入。
      const recordServer = stdioServer("computer-use", { env: { KEEP_ME: "1" } });
      const [recordResolved] = await resolver.resolveMcpServers([recordServer], undefined);
      assert.deepEqual(recordResolved.env, {
        KEEP_ME: "1",
        [BROKER_SOCKET_ENV]: FAKE_SOCKET,
        [PLUGIN_AUTHORITY_ENV]: FAKE_AUTHORITY,
      });
      // env 缺失形态：按 ModeAgentMcpServer 的 {name,value}[] 形态补齐两键。
      const bareServer = { name: "computer-use", command: "uvx", args: ["mode-cua"] };
      const [bareResolved] = await resolver.resolveMcpServers([bareServer], undefined);
      assertInjected(bareResolved.env, FAKE_SOCKET, FAKE_AUTHORITY);
    });
  },
);

test("M5: 非 win32 → resolveMcpServers 直通（platform 覆写）+ 授权重启走 host", async () => {
  await withHome(CUA_ON, async () => {
    const { host, state } = makeHost();
    const resolver = createCuaProductMcpServerResolver(host, {});
    await withPlatform("darwin", async () => {
      const servers = [stdioServer("computer-use")];
      const resolved = await resolver.resolveMcpServers(servers, undefined);
      assert.equal(resolved, servers, "非 win32 必须原样返回");
      assert.equal(state.startCalls, 0, "非 win32 绝不 start");
      assert.equal(
        hasCuaProductHelperAgentEnvUnavailable(host),
        false,
        "非 win32 不落标记（mac 保持现值）",
      );
      // mac 分支的授权后重启：非 win32 不是 no-op，委托 host.restart（brief 语义）。
      await resolver.restartAfterPermissionGrant("onboarding-session");
      assert.equal(
        state.restartCalls,
        1,
        "非 win32 的 restartAfterPermissionGrant 走 host.restart()",
      );
    });
  });
});

test("keep-stub 恒值钉死（每个保留 stub 引用消费方裁决理由）", async () => {
  // createProductCuaHelperHost：仅 services node.ts:1043 darwin 分支消费；mac 一期 fail-closed，
  // 返回 unavailable host（值不变）。
  const macHost = createProductCuaHelperHost({});
  assert.equal(macHost.running, false);
  assert.deepEqual([macHost.socketPath, macHost.pluginAuthority], [null, null]);
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
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
  markCuaProductHelperAgentEnvUnavailable(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), true);
  // 按 host 键隔离：另一个 host 不受影响。
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(makeHost().host), false);
  clearCuaProductHelperAgentEnvUnavailable(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
  // 幂等：重复 mark/clear 不抛。
  for (const op of [
    markCuaProductHelperAgentEnvUnavailable,
    markCuaProductHelperAgentEnvUnavailable,
    clearCuaProductHelperAgentEnvUnavailable,
    clearCuaProductHelperAgentEnvUnavailable,
  ])
    op(host);
  assert.equal(hasCuaProductHelperAgentEnvUnavailable(host), false);
});
// 原第 13 例「CuaHelperError 失败面」已删（fix round 1 / M5）：它 new CuaHelperError 后断言
// isCuaHelperError——测的是 broker.js 自身类语义（broker.test.mjs 已覆盖），对 broker-server.js
// 零覆盖，属同义反复；其「wait 超时用 BrokerError 不改错误类」的意图已由上面
// waitForCuaHelperStartup 的 deadline 用例直接断言。
