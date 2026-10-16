/**
 * Host sidecar 的 stub 服务面（M3 Task 1 的临时实现，Task 2 换成真 `packages/services`）。
 *
 * 纪律（M3 计划的 DR-T1-1）：**每个命令都必须显式失败**。返回 `undefined` 或空对象会让调用方
 * 把「还没实现」读成「成功但没数据」——那正是这个仓库反复吃亏的那类静默失败。因此：
 *
 * - 每个服务频道注册一个 stub，`call(command)` 一律拒绝，错误文案里带**频道名与命令名**；
 * - 错误对象带 `code: "not_implemented"`，调用方既能读文案也能读码；
 * - `listen` 同样显式抛错，不给「订阅成功但永不发事件」的假象。
 *
 * 为什么 stub 是「按频道」而不是「按方法」：`IServiceAccessor` 有 40+ 个服务接口、方法数百个，
 * 逐个手抄一份方法清单本身就是漂移源（Task 2 换上真 `IServiceAccessor` 时，
 * 命令清单应由真对象派生，见 `docs/specs/tauri-renderer-host.md` 的「服务面所有权」）。
 */
import type { IServerChannel } from "@mode/rpc";
import { ServiceChannels } from "@mode/shared";

/** 明确的「未实现」错误码。 */
export const NOT_IMPLEMENTED_CODE = "not_implemented";

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

/** 一个服务频道的 stub：任何命令都显式拒绝。 */
export function createStubServiceChannel(channelName: string): IServerChannel<string> {
  return {
    async call(_ctx, command) {
      throw notImplementedError(channelName, command);
    },
    listen(_ctx, event) {
      throw notImplementedError(channelName, `${event}（listen）`);
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
