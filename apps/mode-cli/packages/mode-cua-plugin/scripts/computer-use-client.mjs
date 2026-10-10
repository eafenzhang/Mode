/**
 * Computer Use 薄 SDK（自研）：方法名常量 + bridge 绑定 + 参数构造辅助。
 * 纯 ESM、零依赖——kernel 可从插件根动态 import（与 browser-client 同机制），
 * 也可在任何拿得到 bridge 的地方 createComputerUseClient({ bridge }) 直接用。
 *
 * 契约见 docs/computer-use.md（工具面规范源）与
 * docs/specs/computer-use-plugin-distribution.md（分发与内容契约）。
 */

/** 14 个方法名（与 runtime 分发面及 docs 机械对照）。 */
export const COMPUTER_METHOD_NAMES = Object.freeze([
  "list_apps",
  "list_windows",
  "get_app_state",
  "left_click",
  "scroll",
  "left_click_drag",
  "type",
  "set_value",
  "select_text",
  "key",
  "perform_action",
  "paste",
  "request_access",
  "stop_computer_control",
]);

/** 只读四方法：不进 controller lease 闸门，stop 之后仍开放。 */
export const READONLY_METHOD_NAMES = Object.freeze([
  "get_app_state",
  "list_apps",
  "list_windows",
  "request_access",
]);

/** 变更十方法：受租约与停止闸门约束（stop 自身幂等放行）。 */
export const MUTATING_METHOD_NAMES = Object.freeze(
  COMPUTER_METHOD_NAMES.filter((name) => !READONLY_METHOD_NAMES.includes(name)),
);

const CUA_BRIDGE_SYMBOL = "mode.node-repl.computer-use-bridge";

/**
 * 取 node_repl kernel 注入的 CUA bridge。缺 bridge 通常意味着：
 * 电脑控制开关未开、不在主会话、或当前宿主不提供 CUA（macOS/Linux 一期）。
 */
export function resolveCuaBridge(scope = globalThis) {
  const bridge = scope[Symbol.for(CUA_BRIDGE_SYMBOL)];
  if (!bridge || typeof bridge.call !== "function") {
    throw new Error(
      "Computer Use bridge 不可用：确认设置里「电脑控制」已开启，且处于主会话（子代理不可用）。",
    );
  }
  return bridge;
}

export function isComputerUseMethod(name) {
  return COMPUTER_METHOD_NAMES.includes(name);
}

export function isReadOnlyComputerUseMethod(name) {
  return READONLY_METHOD_NAMES.includes(name);
}

/**
 * 绑定 bridge 的客户端。只做方法名校验与调用转发——参数语义以 docs/computer-use.md
 * 为准，SDK 不做二次发明（get_app_state 的 strict 四键与动作工具的忽略语义由 runtime 复检）。
 */
export function createComputerUseClient(options = {}) {
  const bridge = options.bridge ?? resolveCuaBridge(options.scope);
  return {
    bridge,
    documentationRoot: bridge.documentationRoot,
    assertAvailable() {
      bridge.assertAvailable?.();
    },
    call(method, input = {}) {
      if (!isComputerUseMethod(method)) {
        throw new Error(
          `Unknown computer-use method: ${method}. Valid methods: ${COMPUTER_METHOD_NAMES.join(", ")}`,
        );
      }
      return bridge.call(method, input);
    },
    /** 观察入口糖：等价 call("get_app_state", …)，四键原样透传。 */
    observe(appRef, extra = {}) {
      return this.call("get_app_state", { app_ref: appRef, ...extra });
    },
    /** 结束并释放租约（幂等）。 */
    stop() {
      return this.call("stop_computer_control", {});
    },
  };
}
