/**
 * 派发边界守卫（M3 Task 2 的核心安全网）。
 *
 * ## 为什么需要它（Task 1 的 C1，现在从「我们的 stub」变成「别人的实现」）
 *
 * `ChannelServer.onEventListen` 调 `channel.listen(ctx, event, arg)` 时**没有 try/catch**
 * （对照 `onPromise` 会 `try { channel.call } catch { Promise.reject }`）。真服务面由
 * `ProxyChannel.fromService` 包装，它的 `listen` 对未知事件名**直接抛** `Event not found`
 * （`packages/rpc/src/proxy-channel.ts`）。于是「一次坏订阅」=「Host 进程当场死亡」，
 * 同端口上在途 RPC 全部变 `transport_closed`，而渲染层一挂载就订阅服务事件。
 *
 * 事件名在真服务面里**不由本仓库的一个地方决定**：`IServiceAccessor` 上有若干**可选**服务
 * （`cuaPermissionService?` / `windowControllerService?` / `conversationMirrorService?` …），
 * 远端与本地提供的面不同，而渲染层按 host 无关的方式订阅——所以「未知事件名」不是异常输入，
 * 是**正常可能的输入**。它不能杀死进程。
 *
 * ## 为什么守卫装在这里而不是逐个服务
 *
 * 装在两处**唯一的**边界上，一次生效于全部服务：
 *
 * 1. `registerChannel`：每个频道在进 `ChannelServer` 之前被包一层，`listen` 的同步抛出被
 *    拦下 → 记数 + 留痕 + 返回**永不发事件**的 Event（不假装有事件，与 Task 1 的 stub 同一纪律）；
 * 2. `dispatch`：入口把 `Emitter.fire(...)`（即 `ChannelServer.onRawMessage` 的调用点）
 *    整个包起来 —— 协议层、序列化层、任何一层的同步异常都只被记账，不会再变成 uncaught
 *    exception 带走进程。
 *
 * 「逐个服务打补丁」在这个仓库是错的：服务有 40+ 个，补丁会有 40+ 个漂移点，而且
 * `ProxyChannel` 才是那个抛错的地方——补在服务上根本拦不住。
 *
 * ## 已知代价（不藏）
 *
 * `listen` 被拦下时调用方**看不出**订阅失败（`IServerChannel.listen` 同步返回 Event，
 * 没有错误通道，而 `IChannelServer` 只暴露 `registerChannel`/`ready`，外部无法替这条请求
 * 回一个 RPC 错误）。所以这里选的是「安全降级 + 可观察」：不投假事件、stderr 留痕、
 * 计数进 `host.describe`。要把它变成客户端的 RPC 错误，得改 `packages/rpc` 的
 * `onEventListen`（那是 Electron 路径共用的包，本任务不改）——登记在 spec 里。
 */
import { toDisposable, type Event, type IServerChannel } from "@mode/rpc";

/** 永不发事件的 Event：`ChannelServer` 拿它注册 fire 回调，回调永不被调用。 */
export const NEVER_FIRING_EVENT: Event<never> = () => toDisposable(() => undefined);

/** 守卫样本的保留条数（诊断用，不无限增长）。 */
const MAX_SAMPLES = 8;

export interface DispatchGuardSnapshot {
  /** `listen` 守卫拦下的订阅次数。 */
  guardedSubscriptions: number;
  /** 派发层守卫拦下的同步异常次数。 */
  dispatchGuards: number;
  /** 被拦下的样本（`频道.事件` / 异常摘要）。 */
  samples: string[];
}

/** 一次 Host 生命周期内的守卫登记处（所有服务共用一份，计数才能是全局事实）。 */
export class DispatchGuards {
  #guardedSubscriptions = 0;
  #dispatchGuards = 0;
  #samples: string[] = [];

  #record(label: string): void {
    this.#samples.push(label);
    if (this.#samples.length > MAX_SAMPLES) this.#samples.shift();
  }

  /**
   * 包一层频道：`listen` 的同步抛出降级为「永不发事件 + 留痕 + 计数」。
   *
   * `call` 原样转交：`ChannelServer.onPromise` 自己有 try/catch（把同步抛出变成
   * `Promise.reject`），在它外面再套一层只会改变错误的传播形态，没有收益。
   */
  guardChannel(
    channelName: string,
    channel: IServerChannel<string>,
    log: (line: string) => void,
  ): IServerChannel<string> {
    return {
      call: (ctx, command, arg, cancellationToken) =>
        channel.call(ctx, command, arg, cancellationToken),
      listen: (ctx, event, arg) => {
        try {
          return channel.listen(ctx, event, arg);
        } catch (error) {
          this.#guardedSubscriptions += 1;
          const label = `${channelName}.${event}: ${error instanceof Error ? error.message : String(error)}`;
          this.#record(label);
          // 每次拦下都留一行（不去重）：同一频道不同事件、以及「一次挂载里订阅了很多次」都是事实，
          // 去重会让「渲染层到底订阅了多少次」这个排障信息消失；样本数组另有界（`MAX_SAMPLES`）。
          log(`listen_guard ${label}`);
          return NEVER_FIRING_EVENT as Event<never>;
        }
      },
    };
  }

  /**
   * 派发入口守卫：把一次同步抛出变成**可诊断的事实**，而不是带走进程的未捕获异常。
   *
   * 返回 `undefined` 表示被拦下了。抛出的请求不会有应答（`IChannelServer` 没有给外部
   * 回错误的能力），调用方表现为 RPC 超时——比「Host 崩掉、所有在途请求一起变
   * `transport_closed`」小得多，而且是**可观察**的。
   */
  dispatch<T>(what: string, action: () => T, log: (line: string) => void): T | undefined {
    try {
      return action();
    } catch (error) {
      this.#dispatchGuards += 1;
      const label = `${what}: ${error instanceof Error ? error.message : String(error)}`;
      this.#record(label);
      log(`dispatch_guard ${label}`);
      return undefined;
    }
  }

  snapshot(): DispatchGuardSnapshot {
    return {
      guardedSubscriptions: this.#guardedSubscriptions,
      dispatchGuards: this.#dispatchGuards,
      samples: [...this.#samples],
    };
  }
}
