/**
 * 真 `IServiceAccessor` 的装配（M3 Task 2 · Step 2）：把 `packages/services` 的**公共入口**
 * （`@mode/services/node` 的 `createLocalServices`）在无 Electron 的 sidecar 进程里跑起来，
 * 并把服务**逐频道**交给 `ChannelServer` —— 每个频道都先过一次派发守卫（`channelGuard.ts`）。
 *
 * ## 为什么走 `createLocalServices` 而不是逐个 `createXxxService`
 *
 * 45 个服务各有自己的构造参数与相互依赖（设置权威、凭据、网络 transport、任务索引 repo …），
 * 手写装配等于把 `packages/services` 的装配知识抄一份到本包——**那份抄写就是漂移源**，
 * 而且违反「不 deep import」的约束（逐个 import 必然牵到 `#src/...` 内部路径）。
 * 用公共入口一次装配，服务之间的依赖关系由它自己的作者维护。
 *
 * ## 为什么用「守卫版 `IChannelServer`」而不是直接暴露
 *
 * `ServiceCollection.exposeOnChannelServer(server, overrides)` 是**唯一**的服务面出口，
 * 它内部对每个服务调 `ProxyChannel.fromService(...)` 再 `registerChannel`。真服务面里
 * `ProxyChannel.listen` 对未知事件名会抛（见 `channelGuard.ts`），而
 * `ChannelServer.onEventListen` 没有 try/catch —— 所以守卫必须装在
 * **`registerChannel` 这个唯一入口**上：传一个包装过的 `IChannelServer` 进去，
 * 每个频道在进真服务器之前被包一层，一次覆盖全部服务（不是逐个打补丁）。
 *
 * ## 与 Electron Host 的装配差异（登记，不藏）
 *
 * Electron Host 在 app 层还传了 `overrides`（窗口级包装）以及 `parentPort`、`settingService`、
 * `hostApiNetworkTransport` 等注入点。本任务只做**服务面本身**：窗口模型、跨窗口广播、
 * 手机远控是 Task 3/4 的范围。缺省注入下这些能力各自表现为「不可用/空操作」，
 * 而不是静默走另一条路径（例如 `parentPort` 传 null 时广播为空操作）。
 */
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

import type { IChannelServer, IServerChannel } from "@mode/rpc";

import { DispatchGuards } from "./channelGuard.js";
import { applyHostResourcesPath } from "./runtimeResources.js";
import type { ScopedAttachmentRegistry } from "./scopedAttachments.js";

const here = dirname(fileURLToPath(import.meta.url));

/**
 * 仓库根（源码形态）。打包形态下这个相对路径**不存在**——那时靠
 * `MODE_BUILTIN_PROVIDER_CONFIG_FILE` 或 `MODE_HOST_RESOURCES_DIR` 指定（见 `resolveBuiltinProviderConfig`）。
 */
const repoRoot = resolve(here, "../../../..");

/** 已登记的仓库既有覆写名（`.env.example` 与 `docs/specs/tauri-agent-runtime.md` 都写着它）。 */
export const BUILTIN_PROVIDER_CONFIG_FILE_ENV = "MODE_BUILTIN_PROVIDER_CONFIG_FILE";

/** 打包形态的资源目录（壳负责设置；Task 5 定落点）。 */
export const HOST_RESOURCES_DIR_ENV = "MODE_HOST_RESOURCES_DIR";

export interface BuiltinProviderConfigResolution {
  path: string;
  /** 命中来源：显式覆写 / 资源目录 / 仓库相对。 */
  source: "env" | "resources" | "repo";
  exists: boolean;
}

/**
 * 解析 Mode Built-in Provider Config 的物理路径。
 *
 * 顺序与仓库既有实现的**语义**一致（显式覆写优先 → 打包资源目录 → 开发态仓库相对），
 * 但**不 import** `packages/desktop/src/main/desktopProviderConfig.ts`：那份实现 import 了
 * `electron`，在 sidecar 里不可用（本包存在的理由之一就是「无 Electron」）。
 * 命中来源要一起报出来——三个候选都可能给出一个「看起来对」的路径，只有来源能定位配置错误。
 */
export function resolveBuiltinProviderConfig(
  env: NodeJS.ProcessEnv = process.env,
): BuiltinProviderConfigResolution {
  const relative = join("config", "provider", "mode-builtin.json");
  const explicit = env[BUILTIN_PROVIDER_CONFIG_FILE_ENV]?.trim();
  if (explicit) return { path: explicit, source: "env", exists: existsSync(explicit) };
  const resources = env[HOST_RESOURCES_DIR_ENV]?.trim();
  if (resources) {
    const path = join(resources, relative);
    return { path, source: "resources", exists: existsSync(path) };
  }
  const path = join(repoRoot, relative);
  return { path, source: "repo", exists: existsSync(path) };
}

export interface RegisteredServiceChannel {
  channelName: string;
  channel: IServerChannel<string>;
}

export interface HostServiceSurface {
  /** 全部服务频道（已套 `listen` 守卫），交给入口逐个注册到真 `ChannelServer`。 */
  channels: RegisteredServiceChannel[];
  channelNames: string[];
  builtinProviderConfig: BuiltinProviderConfigResolution;
  /** 释放真实服务持有的资源（后台任务、子进程、文件句柄）。 */
  dispose(): Promise<void>;
}

export interface CreateHostServiceAccessorOptions {
  guards: DispatchGuards;
  log: (line: string) => void;
  env?: NodeJS.ProcessEnv;
  /** Host 入口持有的 scoped attachment 登记表（LR-4）。 */
  scopedAttachments: ScopedAttachmentRegistry;
}

/**
 * 装配真服务面。**任何抛出都向上传播**：启动失败必须让入口能报出来（要么退出让壳推
 * `failed`，要么在诊断里带出原因），不能在这里吞掉变成「半可用的服务面」。
 */
export async function createHostServiceSurface(
  options: CreateHostServiceAccessorOptions,
): Promise<HostServiceSurface> {
  const env = options.env ?? process.env;
  // 纯 Node sidecar 没有 Electron 注入的 `process.resourcesPath`；先桥接再装配服务，
  // 让 CUA runtime / 随包工具的打包态解析能与 Electron 宿主得到同一个基准目录。
  const resourcesPath = applyHostResourcesPath(env);
  if (resourcesPath) {
    options.log(`host_resources_path=${resourcesPath}`);
  }
  const builtinProviderConfig = resolveBuiltinProviderConfig(env);

  // 动态 import：必须在 stdout 守卫装好**之后**才装载 services（服务在模块初始化/构造期会
  // `console.log`，静态 import 会跑在守卫安装之前——实测 `cua-pip-session` 就是这种写入）。
  const { createLocalServices, disposeServiceResourcesAndWait } = await import("@mode/services/node");
  // LAN/远程桌面/远程工作区管理面（LR-1/LR-2/LR-4）：descriptor 在浏览器安全的根入口，
  // 实现是 Node-only 的本包模块。全部动态装载，避免绕过 stdout 守卫。
  const [
    { ServiceCollection, ILanAccessService, IRemoteDesktopService, IRemoteService },
    { createLanAccessHostService },
    { createRemoteDesktopHostService },
    { createRemoteWorkspaceHostService },
  ] = await Promise.all([
    import("@mode/services"),
    import("./lanAccessService.js"),
    import("./remoteDesktopService.js"),
    import("./remoteWorkspaceService.js"),
  ]);

  // 远程桌面被控开关是 CUA Helper 的创建准入之一（插件关闭但被控打开时仍需 raw_input 通道）。
  // server 在 services 之后才建，所以用闭包现取；与 Electron Host 的装配口径一致。
  let remoteDesktopCapability: { isServerEnabled(): boolean } | null = null;
  const services = createLocalServices({
    modeBuiltinProviderConfigFilePath: builtinProviderConfig.path,
    isRemoteDesktopServerEnabled: () => remoteDesktopCapability?.isServerEnabled() ?? false,
  });

  // **local-only 集合**：管理面只进窗口 renderer 的 ChannelServer，绝不进
  // `startLanAccessServer` 拿到的普通服务集合——那份集合会被 `createHttpServer` 暴露给每个
  // LAN 对端（docs/specs/tauri-renderer-host.md §21.2）。
  const localOnly = new ServiceCollection();
  let lanAccess: Awaited<ReturnType<typeof createLanAccessHostService>> | null = null;
  // 远程桌面 server 先建：LAN 握手需要它的能力位与 hooks；它读 client 记录时再经闭包取 LAN。
  const remoteDesktop = await createRemoteDesktopHostService({
    services,
    log: options.log,
    getActiveLanAccessServer: () => lanAccess?.getActiveServer() ?? null,
  });
  remoteDesktopCapability = remoteDesktop;
  lanAccess = await createLanAccessHostService({
    services,
    log: options.log,
    getRemoteDesktop: () => remoteDesktop,
  });
  const remote = await createRemoteWorkspaceHostService({
    services,
    log: options.log,
    scopedAttachments: options.scopedAttachments,
    guardChannel: (channelName, channel) =>
      options.guards.guardChannel(channelName, channel, options.log),
  });
  localOnly.register(ILanAccessService, lanAccess);
  localOnly.register(IRemoteDesktopService, remoteDesktop);
  localOnly.register(IRemoteService, remote);

  const channels: RegisteredServiceChannel[] = [];
  const guardedServer: IChannelServer<string> = {
    registerChannel(channelName, channel) {
      channels.push({
        channelName,
        channel: options.guards.guardChannel(channelName, channel, options.log),
      });
    },
  };
  services.exposeOnChannelServer(guardedServer);
  localOnly.exposeOnChannelServer(guardedServer);

  return {
    channels,
    channelNames: channels.map((entry) => entry.channelName),
    builtinProviderConfig,
    async dispose() {
      // 先停局域网监听（UDP/HTTP），再收被控会话与远程工作区，最后释放普通服务资源：
      // 否则服务销毁后监听/会话仍可能接单。
      await lanAccess.dispose();
      await remoteDesktop.dispose();
      await remote.disposeHost();
      await disposeServiceResourcesAndWait(services);
    },
  };
}
