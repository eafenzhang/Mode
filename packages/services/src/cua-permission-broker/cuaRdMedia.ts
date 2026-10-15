// 远程桌面（LAN）媒体面注入端口：经既有 CUA Helper 通道调用 broker rd_media_* 方法
// （docs/specs/lan-remote-desktop.md「接口变更」第 5 条 / 状态所有者表：媒体面 = 既有 CUA Helper）。
//
// 本层刻意最薄，边界如下（勿在此扩张语义）：
// - 只做一次调用：不重试（P2 语义：媒体面失败由被控 host 桥决定后续——只 warn、不断会话，
//   不靠超时掩盖；对照 cuaRawInput 的同一「实时链路不做重试」口径）；
// - 不解析 rect/key/salt 与建流语义（helper/Rust 契约已定），不复刻逐参数校验避免两处漂移；
// - 不持有 Helper 生命周期：socket 由解析点提供，宿主装配侧（node.ts）复用 raw_input 同一解析链
//   （live peek → 稳定探测 → 被控开关打开时经同一创建入口按需拉起）；解析不到即 fail closed
//   抛 helper_unavailable，不落本地兜底。
import { BrokerError, CuaHelperError, callBrokerMethod } from "@mode/cua/broker";

/**
 * 注入端口签名：一次 rd_media 调用。
 * method 联合与 host 侧 RemoteDesktopMediaCall 一致（rd_media_start/switch/stop，napi 面）；
 * params 是 helper 契约参数，本层不复刻逐字段校验。
 */
export type CuaRdMediaController = (
  method: "rd_media_start" | "rd_media_switch" | "rd_media_stop",
  params: object,
) => Promise<void>;

export interface CreateCuaRdMediaControllerOptions {
  /** 注入 socket 解析点；返回 null/undefined 表示 helper 不可达（只报错，不拉起、不重试）。 */
  resolveSocketPath: () => string | null | undefined | Promise<string | null | undefined>;
  /** broker 调用实现（测试注入点）；缺省 @mode/cua 的 callBrokerMethod。 */
  callBrokerMethod?: (args: {
    socketPath: string;
    method: string;
    params?: unknown;
    timeoutMs?: number;
  }) => Promise<unknown>;
  /**
   * 单次媒体调用预算。napi 建流（rd_media_start）最长 ~10s，留余量；switch/stop 共用同一
   * 预算（同一控制器出口，不按 method 分叉，避免三套超时口径）。
   */
  timeoutMs?: number;
}

/** rd_media_start 的 napi 建流最长 ~10s + 余量；switch/stop 同预算。 */
const RD_MEDIA_TIMEOUT_MS = 15_000;

// 连接层失败（无 helper / helper 已退出 / 无响应 / 响应不可解析）统一收敛为 helper_unavailable；
// helper 已明确应答的协议错误保留原码（与 cuaRawInput 同一收敛口径），
// 让调用方能区分「helper 不可达」与「helper 拒绝本次媒体指令」。
const UNREACHABLE_BROKER_CODES = new Set([
  "stale_socket",
  "timeout",
  "unavailable",
  "broker_unavailable",
]);

function toRdMediaError(error: unknown): CuaHelperError {
  if (error instanceof CuaHelperError) return error;
  if (error instanceof BrokerError) {
    return new CuaHelperError(error.message, {
      code: UNREACHABLE_BROKER_CODES.has(error.code) ? "helper_unavailable" : error.code,
    });
  }
  return new CuaHelperError(error instanceof Error ? error.message : String(error), {
    code: "helper_unavailable",
  });
}

/**
 * callBrokerMethod 的真实失败形状是**抛 BrokerError**（连接层码 timeout/stale_socket/internal，
 * 或 helper 应答错误码），不是返回 {ok:false}——这里用 try/catch 收敛，与 cuaRawInput 同构。
 */
export function createCuaRdMediaController(
  options: CreateCuaRdMediaControllerOptions,
): CuaRdMediaController {
  const resolveSocketPath = options.resolveSocketPath;
  const invokeBroker = options.callBrokerMethod ?? callBrokerMethod;
  const timeoutMs = options.timeoutMs ?? RD_MEDIA_TIMEOUT_MS;
  return async (method, params) => {
    let socketPath: string | null | undefined;
    try {
      socketPath = await resolveSocketPath();
    } catch (error) {
      // 解析点自身抛错同样收敛为稳定码，不让调用方看到裸错误。
      throw toRdMediaError(error);
    }
    if (!socketPath) {
      throw new CuaHelperError("Computer Use Helper is not reachable for rd_media", {
        code: "helper_unavailable",
      });
    }
    try {
      await invokeBroker({ socketPath, method, params, timeoutMs });
    } catch (error) {
      throw toRdMediaError(error);
    }
  };
}
