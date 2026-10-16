/**
 * Host sidecar 的 stub 服务面（M3 Task 1 的临时实现，Task 2 换成真 `packages/services`）。
 *
 * 纪律（M3 计划的 DR-T1-1）：**每个命令都必须显式失败**。返回 `undefined` 或空对象会让调用方
 * 把「还没实现」读成「成功但没数据」——那正是这个仓库反复吃亏的那类静默失败。因此：
 *
 * - 每个服务频道注册一个 stub，`call(command)` 一律拒绝，错误文案里带**频道名与命令名**；
 * - 错误对象带 `code: "not_implemented"`，调用方既能读文案也能读码；
 * - **`listen` 绝不能抛错**（见下），它返回一个永不发事件的 Event，并把拒绝记在
 *   stderr 与 `host.describe` 的计数里——不假装有事件，也不静默吞掉。
 *
 * ## 为什么 `listen` 不能抛（2026-10-16 实测，别再犯）
 *
 * `ChannelServer.onEventListen`（`packages/rpc/src/channelServer.ts`）调
 * `channel.listen(ctx, event, arg)(callback)` 时**没有 try/catch**——不像 `onPromise`
 * 会 `try { channel.call(...) } catch { Promise.reject(error) }`（同文件 `:109-115`）。
 * 于是 `listen` 里抛出的异常会一路穿出 `onRawMessage` → `Emitter.fire` → stdin 的 `data`
 * 处理函数，变成 uncaught exception：**Host 进程当场死亡**，同一端口上所有在途 RPC 变成
 * `transport_closed`。渲染层一挂载就会订阅服务事件，所以「stub 在 listen 里抛错」等于
 * 「闸门放行后几秒 Host 就死」，而当时的用例只探 `call`，整套测试与 CI 全绿。
 *
 * 上游的这处不对称（`onPromise` 有守卫、`onEventListen` 没有）是 `packages/rpc` 的行为，
 * 本任务不改它——所以约束落在**每个 `IServerChannel` 实现**身上，也写在
 * `docs/specs/tauri-renderer-host.md` 的「未覆盖 / 危险面」里：日后真服务若在 `listen` 里抛，
 * 同样会杀掉 Host；那时修的位置是壳/传输侧（给 dispatch 加守卫），不是逐个服务打补丁。
 *
 * 为什么 stub 是「按频道」而不是「按方法」：`IServiceAccessor` 有 40+ 个服务接口、方法数百个，
 * 逐个手抄一份方法清单本身就是漂移源（Task 2 换上真 `IServiceAccessor` 时，
 * 命令清单应由真对象派生，见 `docs/specs/tauri-renderer-host.md` 的「服务面所有权」）。
 */
import { toDisposable, type Event, type IServerChannel } from "@mode/rpc";
import { ServiceChannels } from "@mode/shared";

/** 明确的「未实现」错误码。 */
export const NOT_IMPLEMENTED_CODE = "not_implemented";

/** 永不发事件的 Event：`ChannelServer` 会立刻拿它注册 fire 回调，回调永不被调用。 */
const neverFiringEvent: Event<never> = () => toDisposable(() => undefined);

/** 被订阅过的次数（对 `host.describe` 可见，便于运维与用例观察 stub 的真实用法）。 */
let eventSubscriptions = 0;

/** 已留过痕的 `(频道, 事件)` 组合：同一处只报一次，避免渲染层反复订阅时刷屏。 */
const reportedSubscriptions = new Set<string>();

/** stub 上「事件订阅」的累计次数。 */
export function stubEventSubscriptions(): number {
  return eventSubscriptions;
}

/** 全部服务频道名：真源是 `packages/shared/src/channels.ts` 的 `ServiceChannels`，不手抄。 */
export function serviceChannelNames(): string[] {
  return Object.values(ServiceChannels);
}

/** 显式的未实现错误：文案点名频道与命令，`code` 供只读码的调用方判断。 */
export function notImplementedError(channelName: string, command: string): Error {
  return Object.assign(
    new Error(
      `${NOT_IMPLEMENTED_CODE}: ${channelName}.${command} 尚未实现` +
        `（M3 Task 1 只打通端口与闸门；真服务面见 Task 2）`,
    ),
    { code: NOT_IMPLEMENTED_CODE, channel: channelName, command },
  );
}

/** 一个服务频道的 stub：任何命令都显式拒绝，事件订阅返回「永不发事件」并留痕。 */
export function createStubServiceChannel(channelName: string): IServerChannel<string> {
  return {
    async call(_ctx, command) {
      throw notImplementedError(channelName, command);
    },
    listen(_ctx, event) {
      // 不抛错：抛了会杀掉 Host（见模块文档的实测）。这里记录 + 返回永不发事件的 Event。
      eventSubscriptions += 1;
      const key = `${channelName}.${event}`;
      if (!reportedSubscriptions.has(key)) {
        reportedSubscriptions.add(key);
        process.stderr.write(
          `[mode-host-sidecar] ${NOT_IMPLEMENTED_CODE}: ${channelName}.${event}（listen）尚未实现` +
            `——stub 不投事件，也不抛错（listen 抛错会杀死 Host，见本文件的模块文档）\n`,
        );
      }
      return neverFiringEvent;
    },
  };
}

/** 每个服务频道一个 stub，交给入口逐个注册到 `ChannelServer`。 */
export function createStubServiceChannels(): Array<{
  channelName: string;
  channel: IServerChannel<string>;
}> {
  return serviceChannelNames().map((channelName) => ({
    channelName,
    channel: createStubServiceChannel(channelName),
  }));
}
