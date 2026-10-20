/**
 * 窗口 renderer 端口上的 v4 可信连接作用域。
 *
 * 背景：`IModeAgentService` 的附件事务（begin/chunk/commit/abort）、附件读取与
 * `conversationRowsRangeV4` 都要求参数携带 `__modeTrustedV4Connection` 载体；
 * 该载体只能由 `createModeAgentConnectionScope` 在 host 侧注入。旧 Electron Host 在
 * `exposeServicesOnMessagePort` 里为每个窗口端口建了这层作用域，Tauri Host sidecar
 * 迁移时漏掉，导致本机工作区经渲染层上传的附件全部以
 * `fault.attachment.connectionUntrusted` 失败（见 docs/specs/tauri-renderer-host.md §11.1）。
 *
 * 本模块只负责「把 base service 包成可暴露给窗口端口的作用域」，不复制任何业务逻辑。
 * services 的运行时值仍由调用方动态 import（services 模块初始化会写 stdout，必须在
 * stdout 守卫装好之后装载），这里只保留 type import。
 */
import { randomUUID } from "node:crypto";
import type {
  IConversationShareService,
  IModeAgentService,
  ModeAgentConnectionScope,
} from "@mode/services";

export interface RendererConnectionScopeOptions {
  agentService: IModeAgentService | undefined;
  conversationShareService?: IConversationShareService;
  /** wire 名由调用方的动态 import 提供，本模块不静态装载 services。 */
  agentChannelName: string;
  conversationShareChannelName: string;
  /** `createModeAgentConnectionScope`（调用方从 `@mode/services` 动态取得）。 */
  createConnectionScope(
    agentService: IModeAgentService,
    context: { connectionId: string; clientMode: "desktop-continuous" },
  ): ModeAgentConnectionScope;
  /** `conversationShareConnectionScopeFactory`（`@mode/services/node` 导出）。 */
  conversationShareScopeFactory: symbol;
  /** 测试注入点；生产缺省 `host-rpc-<uuid>`。 */
  connectionId?: string;
}

export interface RendererConnectionScope {
  /** `exposeOnChannelServer` 的 overrides；空 Map 表示 agent service 缺席。 */
  overrides: Map<string, unknown>;
  dispose(): Promise<void>;
}

type ShareScopeFactory = (agentService: IModeAgentService) => IConversationShareService;

function readShareScopeFactory(
  service: IConversationShareService,
  factory: symbol,
): ShareScopeFactory | null {
  const candidate = (service as unknown as Record<symbol, unknown>)[factory];
  if (typeof candidate !== "function") {
    return null;
  }
  // 符号方法用 call 绑定原 service 的 this，等价于旧 Electron Host 的 `service[factory](…)`。
  return (agentService) =>
    (
      candidate as (
        this: IConversationShareService,
        agent: IModeAgentService,
      ) => IConversationShareService
    ).call(service, agentService);
}

export function createRendererConnectionScope(
  options: RendererConnectionScopeOptions,
): RendererConnectionScope {
  const overrides = new Map<string, unknown>();
  const { agentService } = options;
  if (!agentService) {
    return { overrides, dispose: async () => undefined };
  }

  const scope = options.createConnectionScope(agentService, {
    connectionId: options.connectionId ?? `host-rpc-${randomUUID()}`,
    clientMode: "desktop-continuous",
  });
  overrides.set(options.agentChannelName, scope.service);

  const shareService = options.conversationShareService;
  const shareFactory = shareService
    ? readShareScopeFactory(shareService, options.conversationShareScopeFactory)
    : null;
  if (shareService && shareFactory) {
    // share 服务内部持有 raw agent；不复用同一 scope 时 rowsRange/附件读取同样 untrusted。
    overrides.set(options.conversationShareChannelName, shareFactory(scope.service));
  }

  return {
    overrides,
    dispose: () => scope.dispose(),
  };
}
