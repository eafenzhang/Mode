// 远程桌面（LAN）被控端输入注入端口：经既有 CUA Helper 通道调用 broker 方法 raw_input
// （docs/specs/lan-remote-desktop.md「接口变更」第 4 条 / 状态所有者表「输入注入 = 既有 CUA Helper」）。
//
// 本层刻意最薄，边界如下（勿在此扩张语义）：
// - 只做一次调用：不重试（实时链路，失败语义由被控 main 桥决定，不靠超时掩盖）；
// - 不做前台/权限校验与 CUA 动作语义（Rust raw_input 已定：无 UIA 预检、无 ensure_foreground）；
// - 不创建 Helper 生命周期、不拉起 Helper：socket 由注入的解析点提供，宿主装配侧复用
//   既有「托管 host 的 live socket → 稳定 socket 探测」发现口径（node.ts）。
import {
  BrokerError,
  CuaHelperError,
  callBrokerMethod,
  resolveBrokerSocketPath,
} from "@mode/cua/broker";

/**
 * 注入端口签名：一次 raw_input 调用。
 * events 形状是 helper/Rust 契约（{k:"move"|"button"|"wheel"|"key"|"text"} 联合），
 * 本层不复刻逐条校验，避免与 Rust parse_raw_input 两处漂移。
 */
export type CuaRawInputInjector = (events: unknown[]) => Promise<{ dispatched: number }>;

export interface CreateCuaRawInputInjectorOptions {
  /**
   * 注入 socket 解析点；返回 null/undefined 表示 helper 不可达（只报错，不拉起、不重试）。
   * 缺省 resolveBrokerSocketPath()：与 services 既有 probeStableCuaHelperSocket 同口径的 env 发现。
   */
  resolveSocketPath?: () => string | null | undefined | Promise<string | null | undefined>;
  /** broker 调用实现（测试注入点）；缺省 @mode/cua 的 callBrokerMethod。 */
  callBrokerMethod?: (args: {
    socketPath: string;
    method: string;
    params?: unknown;
    timeoutMs?: number;
  }) => Promise<unknown>;
  /**
   * 单次注入预算。注入是实时链路：helper 无响应时 fail fast，不把主控输入循环拖住
   * （对照 services 既有按需探测的短预算先例：ping 300ms / permission_status 3s）。
   */
  timeoutMs?: number;
}

const DEFAULT_RAW_INPUT_TIMEOUT_MS = 5_000;

// 连接层失败（无 helper / helper 已退出 / 无响应 / 响应不可解析）统一收敛为 helper_unavailable；
// helper 已明确应答的协议错误保留原码（如 invalid_request / method_not_found），
// 让调用方能区分「helper 不可达」与「helper 拒绝本次注入」。
const UNREACHABLE_BROKER_CODES = new Set([
  "stale_socket",
  "timeout",
  "unavailable",
  "broker_unavailable",
]);

function toInjectError(error: unknown): CuaHelperError {
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

export function createCuaRawInputInjector(
  options: CreateCuaRawInputInjectorOptions = {},
): CuaRawInputInjector {
  const resolveSocketPath = options.resolveSocketPath ?? (() => resolveBrokerSocketPath());
  const invokeBroker = options.callBrokerMethod ?? callBrokerMethod;
  const timeoutMs = options.timeoutMs ?? DEFAULT_RAW_INPUT_TIMEOUT_MS;
  return async (events: unknown[]) => {
    // 只拦数组门（helper/server.mjs 首行同判）；逐条形状归 helper 与 Rust 校验。
    if (!Array.isArray(events)) {
      throw new CuaHelperError("raw_input events must be an array", { code: "invalid_request" });
    }
    let socketPath: string | null | undefined;
    try {
      socketPath = await resolveSocketPath();
    } catch (error) {
      // 解析点自身抛错同样收敛为稳定码，不让调用方看到裸错误。
      throw toInjectError(error);
    }
    if (!socketPath) {
      throw new CuaHelperError("Computer Use Helper is not reachable for raw input", {
        code: "helper_unavailable",
      });
    }
    let result: unknown;
    try {
      result = await invokeBroker({
        socketPath,
        method: "raw_input",
        params: { events },
        timeoutMs,
      });
    } catch (error) {
      throw toInjectError(error);
    }
    const dispatched = (result as { dispatched?: unknown } | null | undefined)?.dispatched;
    if (typeof dispatched !== "number") {
      // helper 有应答但不是 raw_input 形状（版本不匹配等）：按不可用 fail closed。
      throw new CuaHelperError("Computer Use Helper returned an invalid raw_input result", {
        code: "helper_unavailable",
      });
    }
    return { dispatched };
  };
}
