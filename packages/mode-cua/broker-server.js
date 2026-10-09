import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { BrokerError, BROKER_SOCKET_ENV, CuaHelperError } from "./broker.js";

export const HELPER_ADDON_ENV = "MODE_CUA_HELPER_ADDON";
export const WINDOWS_DEV_CONTROL_PROTOCOL = "mode-cua-windows-dev/v1";
// helper↔runtime 协议版本（helper health 响应携带 protocolVersion 字段）：Plan B runtime
// 连接后首调比对，不一致 → 向 SDK 吐 version_mismatch；host 的 parseReadyMessage 忽略附加字段。
export const HELPER_PROTOCOL_VERSION = "1.0";

const UNAVAILABLE = "Computer Use is not available in this build.";

function unavailableReject() {
  return Promise.reject(new CuaHelperError(UNAVAILABLE));
}

// 保留 stub：唯一消费方是 services node.ts:1845 的 launchStandaloneCuaHelperForStatus
// （darwin getStatus 状态链，node.ts:1914 shouldUseCuaPermissionService 仅 darwin 生效）。
// mac 一期 fail-closed——不产出 dev argv，空参数即现值。
export function buildHelperOpenArgs(_spec, _launcherPid) {
  return [];
}

export async function resolveHelperPermissionSubjectIdentity(_appPath) {
  throw new CuaHelperError(UNAVAILABLE);
}

// 保留 stub：唯一消费方是 services node.ts:1849（同上 darwin 状态链的 dev 判定）。
// 恒 false = 生产形态（不追加 unsigned-launcher/external-escape argv），mac 一期 fail-closed。
export function isCuaLocalDevelopmentRuntime(_env, _compiledLocalDevelopmentRuntime) {
  return false;
}

export function createCuaHelperInstaller(_options) {
  return {
    ensureInstalled: unavailableReject,
    verifyInstalled: unavailableReject,
  };
}

export const defaultCuaHelperVerifierDependencies = {
  readExecutableArchs: unavailableReject,
  verifyCodeSignature: unavailableReject,
  verifyTeamIdentifier: unavailableReject,
};

// 保留 stub：refresh-marker 对在本仓零调用方（rg 全仓仅定义处）。链路两端都休眠——
// 写端没人把 MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER 放进 agent spawn env
// （buildCuaProductHelperAgentEnv 只下发 socket+authority），读端 node-repl-host:381 透传给
// createComputerUseRuntime 后 index.js:1270 注释自证尚未消费 refreshMarkerPath。
// 待 Task 8/spec 决定 marker 归属后如需再实现（消费方驱动，YAGNI）。
export function cuaBrokerRefreshMarkerPath(_socketPath) {
  return undefined;
}

export async function publishCuaBrokerRefreshMarker(_socketPath, _options) {
  return { path: undefined };
}

export function loadRealNativeAddon(_options) {
  throw new CuaHelperError(UNAVAILABLE);
}

export function resolvePackagedNativeAddonPath(_options) {
  return undefined;
}

export function resolveInTreeAddonPath(_options) {
  return undefined;
}

export function createAxReadOnlyMethods(_source, _registry, _options) {
  return {};
}

export const ROLE_TO_KIND = {};

export function roleToKind(_role) {
  return undefined;
}

export class CuaHelperLifecycleManager {
  #dispose;
  #current;
  #disposed = false;
  constructor(dispose) {
    this.#dispose = dispose;
    this.#current = undefined;
  }
  async acquire(options) {
    if (typeof options?.isAdmitted === "function" && !options.isAdmitted()) {
      return undefined;
    }
    const managed = options?.create?.();
    this.#current = managed;
    return managed;
  }
  peek() {
    return this.#current;
  }
  get disposed() {
    return this.#disposed;
  }
  async dispose(managed) {
    this.#disposed = true;
    await this.#dispose?.(managed ?? this.#current);
  }
}

// 保留 stub（值不变）：消费方 services node.ts:1704 创建、:2194/:2206/:2217 三处写入，
// 全仓无任何读方（write-only bookkeeping）——没有读方就没有可实现的语义。
export class CuaProductHelperWorkspaceRegistry {
  setEnabled(_context, _enabled) {}
}

// 保留 stub（返回 unavailable host，值不变）：唯一消费方 services node.ts:1043 只在
// platform === "darwin" 分支调用（createDefaultCuaProductHelper 的 mac 装配）。
// mac 一期 fail-closed：宿主存在但 start/restart 一律不可用，win32 走 createWindowsCuaHelperHost。
export function createProductCuaHelperHost(_options) {
  return createUnavailableCuaHelperHost();
}

function createUnavailableCuaHelperHost() {
  return {
    get running() {
      return false;
    },
    get socketPath() {
      return null;
    },
    get pluginAuthority() {
      return null;
    },
    get reservedTransport() {
      return undefined;
    },
    start: unavailableReject,
    stop: async () => {},
    restart: unavailableReject,
    restartAfterCurrentStart: unavailableReject,
    waitForTransport: unavailableReject,
    checkHealth: unavailableReject,
    queryScreenCaptureProbe: async () => ({
      ok: false,
      reason: UNAVAILABLE,
    }),
    queryScreenRecordingPreflight: async () => undefined,
    queryPermissionStatus: async () => ({}),
  };
}

// 官方 CUA 插件启用态的唯一读取位。消费方（services node.ts:1717 isCuaEnabledForContext）只传
// {env, workingDirectory}，且是同步布尔——本仓没有 enabledPlugins 的 env 快照（rg 无该形态键），
// 与 services 各 reader（subagentsService/skillsService readPluginConfig）同源读
// ~/.mode/cli/config.json 的 plugins.enabledPlugins；判定形状对齐 CLI bootstrap
// （plugins.ts:426/804 的 `?? false`）。
// fail-closed：computer-use 默认关闭（shared/plugin-marketplaces.ts「电脑控制回退为默认关闭」），
// 缺文件/坏 JSON/总开关关/卸载抑制一律 false。id 存量写法按 adapters/src/config/schema.ts
// pluginIdAliases 归一（旧名 mode-cua + 旧市场段 zcode-plugins-official）。
// workingDirectory 仅为签名兼容保留：本仓插件启用态无 workspace 作用域（消费方注释
// node.ts:1711-1715 亦说明 main 的 bootstrap 只按内建门控，不做 workspace enablement）。
const OFFICIAL_CUA_PLUGIN_ID = "computer-use@mode-plugins-official";
const OFFICIAL_CUA_PLUGIN_ID_ALIASES = new Set([
  OFFICIAL_CUA_PLUGIN_ID,
  "computer-use@zcode-plugins-official",
  "mode-cua@mode-plugins-official",
  "mode-cua@zcode-plugins-official",
]);

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isOfficialCuaPluginId(id) {
  return typeof id === "string" && OFFICIAL_CUA_PLUGIN_ID_ALIASES.has(id.trim().toLowerCase());
}

export function isOfficialCuaPluginEnabledForWorkspace(options) {
  const env = options?.env ?? process.env;
  const home = env.HOME?.trim() || homedir();
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(join(home, ".mode", "cli", "config.json"), "utf8"));
  } catch {
    return false;
  }
  if (!isRecord(parsed)) return false;
  const plugins = isRecord(parsed.plugins) ? parsed.plugins : {};
  if (plugins.enabled === false) return false;
  if (
    Array.isArray(plugins.suppressedBuiltins) &&
    plugins.suppressedBuiltins.some(isOfficialCuaPluginId)
  ) {
    return false;
  }
  const enabledPlugins = isRecord(plugins.enabledPlugins) ? plugins.enabledPlugins : {};
  for (const [id, enabled] of Object.entries(enabledPlugins)) {
    if (isOfficialCuaPluginId(id)) return enabled === true;
  }
  return false;
}

// agent spawn 凭据链的进程内 admission 标志（消费方 services node.ts：mark 于 :640/:643/:1288
// start 失败、has 于 :1159 起的 fail-closed 短路、clear 于 :1167 健康探针恢复）。
// 按 host 对象 WeakSet 隔离（d.ts 键类型 Pick<CuaHelperHost, "start">），多 host/代际互不串扰。
// 说明：MODE_CUA_PERMISSION_BROKER_UNAVAILABLE 的 spawn env 值本身在全仓无读方（rg 仅见写处），
// 真正的读方就是下面这三个函数构成的 admission 门。
const cuaProductHelperAgentEnvUnavailable = new WeakSet();

export function markCuaProductHelperAgentEnvUnavailable(host) {
  if (typeof host === "object" && host !== null) cuaProductHelperAgentEnvUnavailable.add(host);
}

export function hasCuaProductHelperAgentEnvUnavailable(host) {
  return typeof host === "object" && host !== null && cuaProductHelperAgentEnvUnavailable.has(host);
}

export function clearCuaProductHelperAgentEnvUnavailable(host) {
  if (typeof host === "object" && host !== null) cuaProductHelperAgentEnvUnavailable.delete(host);
}

// resolver 的凭据注入：只给 mode-cua 候选 server 的 env 定向补 socket + authority。
// 定向注入是 sanctioned 形态——broker 凭据从 agent 全局 env 剔除（shared/runtimeEnv.ts
// SANITIZED_RUNTIME_ENV_KEYS），子进程只能经 `...config.env` 在 buildMcpStdioEnv 之后
// spread 拿到（adapters/mcp/index.ts:1425 注释同源）。绝不写 process.env（confused-deputy）。
// 半组凭据不下发：与 shared/runtimeEnv.ts captureModeCuaBrokerCredentials「socket 与 authority
// 同批出现才构成有效凭据组」同判据。
const MODE_CUA_PLUGIN_AUTHORITY_ENV = "MODE_CUA_PLUGIN_AUTHORITY";

function upsertBrokerEnv(env, socketPath, pluginAuthority) {
  const pairs = [
    [BROKER_SOCKET_ENV, socketPath],
    [MODE_CUA_PLUGIN_AUTHORITY_ENV, pluginAuthority],
  ];
  if (Array.isArray(env)) {
    // ModeAgentMcpServer 的 env 是 {name,value}[]（shared/mcp.ts）；先去旧值再补新值。
    const kept = env.filter((entry) => !pairs.some(([name]) => entry?.name === name));
    return [...kept, ...pairs.map(([name, value]) => ({ name, value }))];
  }
  if (isRecord(env)) {
    return {
      ...env,
      [BROKER_SOCKET_ENV]: socketPath,
      [MODE_CUA_PLUGIN_AUTHORITY_ENV]: pluginAuthority,
    };
  }
  return pairs.map(([name, value]) => ({ name, value }));
}

function injectModeCuaBrokerCredentials(servers, socketPath, pluginAuthority) {
  if (!Array.isArray(servers)) return servers;
  let changed = false;
  const resolved = servers.map((server) => {
    if (!isPotentialModeCuaAgentMcpServer(server)) return server;
    changed = true;
    return { ...server, env: upsertBrokerEnv(server.env, socketPath, pluginAuthority) };
  });
  return changed ? resolved : servers;
}

/**
 * win32 一期 resolver 关键面。凭据主链在消费方侧（services node.ts resolveSpawnEnv →
 * buildCuaProductHelperAgentEnv → modeAgentProcessManager 合入 agent spawn env → CLI 入口
 * sanitizeModeRuntimeEnv 捕获 → bootstrap injectCuaCredentialsIntoNodeRepl），本 resolver 的职责
 * 是 demand boundary：拉起 host 让主链拿到 warm tuple、失败落 fail-closed 标记、并把凭据定向
 * 注入本次 resolve 的 mode-cua 候选 server。mac 一期 fail-closed（platform 门直接原样返回）。
 */
export function createCuaProductMcpServerResolver(host, options) {
  const hasActiveTurn = options?.hasActiveTurn;
  return {
    async resolveMcpServers(servers, context) {
      // ① unavailable 标记未清前短路直返（plan：短路直到显式 clear），失败绝不抛。
      if (hasCuaProductHelperAgentEnvUnavailable(host)) return servers;
      // ② win32 一期：非 win32 原样返回且不 start、不标记（mac 保持 fail-closed 现值）。
      if (process.platform !== "win32") return servers;
      // ③ 官方插件未启用 → 原样返回且不 start。
      if (
        !isOfficialCuaPluginEnabledForWorkspace({
          env: process.env,
          workingDirectory: context?.workspacePath,
        })
      ) {
        return servers;
      }
      // ④ 活跃 CUA turn 中途不拉起/轮换 Helper——hasActiveTurn 就是在这里被调用的
      // （消费方 node.ts:1706-1710 前向引用注释：resolver 调用发生在后续某次 resolveMcpServers）。
      if (hasActiveTurn?.()) return servers;
      let handle;
      try {
        handle = await host.start();
      } catch {
        // 失败 → 落 unavailable 标记（buildCuaProductHelperAgentEnv 同步 fail-closed）+ 原样返回。
        markCuaProductHelperAgentEnvUnavailable(host);
        return servers;
      }
      const socketPath = handle?.socketPath ?? host.socketPath ?? undefined;
      const pluginAuthority = handle?.pluginAuthority ?? host.pluginAuthority ?? undefined;
      if (!socketPath || !pluginAuthority) return servers;
      return injectModeCuaBrokerCredentials(servers, socketPath, pluginAuthority);
    },
    async restart() {
      // 消费方：ICuaPermissionService.restartHelper（node.ts:850/:2066）经 dynamic resolver 委托到这里。
      await host.restart();
    },
    async restartAfterPermissionGrant(_onboardingSessionId) {
      // Windows 无 TCC 授权流：授权后重启直接 no-op（brief 一期语义），绝不抛。
      if (process.platform === "win32") return;
      // mac 授权流走宿主重启；mac 一期 stub host 拒绝即 fail-closed（与旧 stub 同消息）。
      await host.restart();
    },
  };
}

// 有界等待：deadline 先于内部 promise settle → reject BrokerError{code:"broker_unavailable"}
// （plan Task 5 裁决；d.ts 未声明错误语义，node.ts:1268 caller_timeout 分支的差异见任务报告）。
// 内部先 settle 则成功/失败都原样透传（保持同一引用/同一错误）。deadline 缺省 → 恒等透传。
export async function waitForCuaHelperStartup(startup, deadlineMs) {
  if (deadlineMs === undefined || !Number.isFinite(deadlineMs)) return await startup;
  let timer;
  try {
    return await Promise.race([
      startup,
      new Promise((_resolve, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new BrokerError(`Computer Use Helper startup timed out after ${deadlineMs}ms`, {
                code: "broker_unavailable",
              }),
            ),
          deadlineMs,
        );
        timer.unref?.();
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

// 与 packages/shared/src/mcp.ts 的 isModeCuaMcpCommand / isModeCuaMcpPackageArg 同判定
// （单一事实源见 shared/mcp.ts 注释：desktop resolver 与 CLI bootstrap 两条注入入口必须一致）。
// mode-cua 零依赖不能引 @mode/shared，按其源码复制模式串与叶子归一逻辑；改动需两侧同步。
function modeCuaArgLeaf(value) {
  return (
    value
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .pop() ?? value
  );
}

function matchesModeCuaSpec(candidate) {
  const c = candidate.replace(/_/g, "-");
  return (
    c === "mode-cua" ||
    c.startsWith("mode-cua[") ||
    c.startsWith("mode-cua@") ||
    c.startsWith("mode-cua==") ||
    c.startsWith("mode-cua.")
  );
}

function isModeCuaSpec(value) {
  return matchesModeCuaSpec(value) || matchesModeCuaSpec(modeCuaArgLeaf(value));
}

export function isPotentialModeCuaAgentMcpServer(server) {
  if (!isRecord(server)) return false;
  if (server.name === "computer-use") return true;
  if (typeof server.command === "string" && isModeCuaSpec(server.command)) return true;
  if (
    Array.isArray(server.args) &&
    server.args.some((arg) => typeof arg === "string" && isModeCuaSpec(arg))
  ) {
    return true;
  }
  // official plugin 身份标记：adapters/src/plugins/mcp.ts:226 权威写入 MODE_PLUGIN_ID；
  // 同时覆盖 ModeAgentMcpServer 的 env 数组与 McpServerConfig 的 env record 两种形态。
  const env = server.env;
  if (Array.isArray(env)) {
    for (const entry of env) {
      if (entry?.name === "MODE_PLUGIN_ID" && isOfficialCuaPluginId(entry.value)) return true;
    }
  } else if (isRecord(env) && isOfficialCuaPluginId(env.MODE_PLUGIN_ID)) {
    return true;
  }
  return false;
}

// 保留 stub（值不变）：唯一消费方 runCuaScreenCaptureReadinessProbe（node.ts:795）只被 darwin
// 门控的 permission getStatus 调用（node.ts:1914 shouldUseCuaPermissionService 仅 darwin）；
// mac 一期宿主即 unavailable stub（queryScreenCaptureProbe 恒 {ok:false}），现值与恒 false 一致。
export function isScreenCaptureProbeSuccess(_probe) {
  return false;
}

// 保留 stub：唯一消费方 node.ts:1028 在 `platform === "darwin"` 分支调用（mac 孤儿 Helper 回收）。
// mac 一期 fail-closed——回收器不动作；win32 的 per-Helper launcher-pid watchdog 在 helper/entry.mjs。
export async function reapOrphanedHelpers(_options) {}

export async function requestHelperAccessibilityPermissionViaLaunchServices(_options) {
  return { ok: false, reason: UNAVAILABLE };
}

export async function requestHelperScreenRecordingPermissionViaLaunchServices(_options) {
  return { ok: false, reason: UNAVAILABLE };
}
