import { ChannelServer, Emitter } from "@mode/rpc";
import type { Event, FrameDemultiplexer } from "@mode/rpc";

/**
 * scoped attachment 的 Host 侧登记表（LR-4）。
 *
 * - `attach` 只建 scoped `ChannelServer` 并登记，**不 ready**：页面端口尚未注册时发 Initialize
 *   会被解复用按未知 scope 丢弃；
 * - 页面桥在 renderer 注册完 `ChannelClient` 后发 `scope-ready` 控制帧 → `start()`；
 * - `detach` / `disposeAll` 回收路由与 server；`onDidDetach` 让业务层同步清理 attachment 表。
 *
 * 测试入口（`MODE_HOST_TEST_SCOPES=1`）与生产 `remote.attach` 共用这一份实现。
 */
export interface ScopedAttachment {
  scopeId: number;
  start(): void;
  dispose(): void;
}

export interface ScopedAttachmentRegistry {
  /** 建 scope 并交给调用方注册频道；`register` 抛出时自动回收。 */
  attach(register: (server: ChannelServer) => void): ScopedAttachment;
  /** 页面端口就绪：发 Initialize。未知 scope 返回 false。 */
  start(scopeId: number): boolean;
  /** 回收 scope；未知 scope 返回 false。 */
  detach(scopeId: number): boolean;
  /** scope 被回收（业务层同步清理 attachment 表）。 */
  readonly onDidDetach: Event<number>;
  disposeAll(): void;
  count(): number;
}

export interface CreateScopedAttachmentRegistryOptions {
  demux: FrameDemultiplexer;
  log: (line: string) => void;
}

export function createScopedAttachmentRegistry(
  options: CreateScopedAttachmentRegistryOptions,
): ScopedAttachmentRegistry {
  const servers = new Map<number, ChannelServer>();
  const detachEmitter = new Emitter<number>();
  let nextScopeId = 1;

  const remove = (scopeId: number, source: string): boolean => {
    const server = servers.get(scopeId);
    if (!server) {
      return false;
    }
    servers.delete(scopeId);
    options.demux.detachScope(scopeId);
    server.dispose();
    options.log(`scope_detached scope=${scopeId} source=${source}`);
    detachEmitter.fire(scopeId);
    return true;
  };

  return {
    attach(register) {
      const scopeId = nextScopeId++;
      const server = new ChannelServer(
        options.demux.attachScope(scopeId),
        `mode-host-sidecar/scope-${scopeId}`,
        1000,
        true,
      );
      try {
        register(server);
      } catch (error) {
        options.demux.detachScope(scopeId);
        throw error;
      }
      servers.set(scopeId, server);
      options.log(`scope_attached scope=${scopeId}`);
      return {
        scopeId,
        start: () => {
          server.ready();
          options.log(`scope_ready scope=${scopeId}`);
        },
        dispose: () => {
          remove(scopeId, "attachment");
        },
      };
    },
    start(scopeId) {
      const server = servers.get(scopeId);
      if (!server) {
        options.log(`scope_ready_unknown scope=${scopeId}`);
        return false;
      }
      server.ready();
      options.log(`scope_ready scope=${scopeId}`);
      return true;
    },
    detach(scopeId) {
      return remove(scopeId, "control");
    },
    get onDidDetach() {
      return detachEmitter.event;
    },
    disposeAll() {
      for (const scopeId of Array.from(servers.keys())) {
        remove(scopeId, "dispose");
      }
      detachEmitter.dispose();
    },
    count: () => servers.size,
  };
}
