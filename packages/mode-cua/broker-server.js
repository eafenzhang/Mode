import { existsSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
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
// {env, workingDirectory} 且要同步布尔——按 bootstrap 的**合并视图**读，不是只读 user 层：
// ① user 层 ~/.mode/cli/config.json；
// ② project/workspace 层从 workingDirectory 发现的配置文件（root→cwd 合并，后者覆盖前者）。
// 发现与合并的单一事实源（本文件零依赖镜像，改动需两侧同步，drift 风险见任务报告）：
//   - 发现：config-factory.ts:124-125 → project-config.adapter.ts loadProjectConfigs →
//     shared/workspace-hook-config.ts discoverWorkspaceHookConfigPaths / getProjectConfigDirectories
//     （从 workingDirectory 逐级向上到含 .git 的 worktree 根并反转成 root→cwd；找不到 .git 只用
//     workingDirectory 本身；每目录候选 mode.json、再 .mode/config.json，按此序发现、后发现者覆盖）；
//   - 合并：config-merger.ts:79-96（enabledPlugins 按 pluginId 逐键覆盖——workspace 层的键盖掉
//     user 层，见 plugins-command.ts:298「workspace/project 层的 enabledPlugins=true 优先级更高」；
//     suppressedBuiltins 高层出现即整表替换、否则继承低层）。
// 有效读位对齐 CLI bootstrap（plugins.ts:426/804 的 `enabledPlugins[id] ?? false` + 抑制标记）；
// `plugins.enabled` 总开关故不作门：bootstrap 插件加载器不消费它（plugins.ts:426/804 只读
// enabledPlugins）；skills/commands 有读（skillsService.ts:622/794、commandsService.ts:216/365）。
// fail-closed：computer-use 默认关闭（shared/plugin-marketplaces.ts「电脑控制回退为默认关闭」），
// 两层都缺/坏 JSON/被抑制一律 false。id 存量写法按 adapters/src/config/schema.ts pluginIdAliases
// 归一（旧名 mode-cua + 旧市场段 zcode-plugins-official）。之前此处错误声称「本仓无 workspace
// 作用域」——已按 I1 评审修正（contradicting sites 见任务报告 fix round 1）。
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

// ── project 配置发现（镜像 shared/workspace-hook-config.ts:335-357，sync 形态）──
function hasWorktreeMarker(directory) {
  const marker = join(directory, ".git");
  try {
    if (!existsSync(marker)) return false;
    const stats = statSync(marker);
    return stats.isDirectory() || stats.isFile();
  } catch {
    return false;
  }
}

// 含 .git 的目录即 worktree 根：root→cwd 反转；一路上不到 .git 则只用 workingDirectory 本身
// （与 getProjectConfigDirectories 同语义，不退化成"扫到文件系统根"）。
function getProjectConfigDirectories(start) {
  const directories = [];
  let current = start;
  while (true) {
    directories.push(current);
    if (hasWorktreeMarker(current)) return directories.reverse();
    const parent = dirname(current);
    if (parent === current) break;
    current = parent;
  }
  return [start];
}

// 每目录候选顺序 mode.json → .mode/config.json（镜像 buildWorkspaceHookCandidatePaths，
// 同目录内后发现的 .mode/config.json 在合并序里覆盖 mode.json）。
function discoverProjectPluginConfigPaths(workingDirectory) {
  const start = resolve(workingDirectory ?? process.cwd());
  return getProjectConfigDirectories(start).flatMap((directory) => [
    join(directory, "mode.json"),
    join(directory, ".mode", "config.json"),
  ]);
}

// 单层读取：文件缺失/坏 JSON → 跳过该层（对齐 loadFileConfig 的 loaded=false 语义）。
function readPluginConfigLayer(path) {
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return undefined;
  }
  return isRecord(parsed) && isRecord(parsed.plugins) ? parsed.plugins : {};
}

// 逐键合并（镜像 config-merger.ts:79-96 的 enabledPlugins / suppressedBuiltins 分支）。
function mergePluginLayers(previous, next) {
  if (!next) return previous;
  const merged = { ...previous, ...next };
  if (isRecord(next.enabledPlugins)) {
    merged.enabledPlugins = { ...previous?.enabledPlugins, ...next.enabledPlugins };
  } else {
    merged.enabledPlugins = previous?.enabledPlugins;
  }
  return merged;
}

export function isOfficialCuaPluginEnabledForWorkspace(options) {
  const env = options?.env ?? process.env;
  const home = env.HOME?.trim() || homedir();
  // ① user 层（对齐 services readPluginConfig / hasGlobalCliModeCuaServer 的路径与 HOME 解析）。
  let plugins = readPluginConfigLayer(join(home, ".mode", "cli", "config.json"));
  // ② project 层 root→cwd 顺序合并，workingDirectory 层最后（最高优先）。workingDirectory 缺省
  // 落 process.cwd()——与 loadProjectConfigs(workingDirectory ?? process.cwd()) 同默认
  // （modeTaskServiceAdapter 的调用不带 context，镜像消费方的回退而非跳过，避免 I1 同类假阴性）。
  for (const path of discoverProjectPluginConfigPaths(options?.workingDirectory)) {
    plugins = mergePluginLayers(plugins, readPluginConfigLayer(path));
  }
  if (!plugins) return false;
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
      // （消费方 node.ts:1706-1710 前向引用注释：resolver 调用发生在后续某次 resolveMcpServers；
      //   node.ts:2276-2277 只禁「turn 中途回收/重启 Helper」，不禁读取现有 tuple）。
      // M4 裁决：host 已在跑（running）时直接用 host.socketPath/pluginAuthority 注入现有 tuple、
      // 不调 start()；冷 host 才不动作（不 mid-turn 拉起）。unavailable 短路已在 ① 挡住病态 host。
      if (hasActiveTurn?.()) {
        if (host.running) {
          const warmSocket = host.socketPath ?? undefined;
          const warmAuthority = host.pluginAuthority ?? undefined;
          if (warmSocket && warmAuthority) {
            return injectModeCuaBrokerCredentials(servers, warmSocket, warmAuthority);
          }
        }
        return servers;
      }
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
// 真实价值（M2 修正）：为消费方包住 createWindowsCuaHelperHost 的**冷 resolveRuntime** 解析链
// （node.ts:968-978 getHost → options.resolveRuntime——missing-native-addon/artifact-integrity 等
// 可能长挂），node.ts:1216 与 :1248 两处调用都包在这条 wrapper promise 外面。
// 1248 的冷启动 race 分支在本仓不可达：每个 in-repo host 都暴露 waitForTransport
// （Windows wrapper node.ts:970、unavailable stub broker-server.js:146），:1212 判真即走 :1216-1231
// 早返回——故 caller_timeout 分支的差异比任务报告原先评估的更惰性（详见 fix round 1）。
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
