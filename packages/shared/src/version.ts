// 由各 bundler 通过 define 注入，避免运行时 JSON import 的跨 bundler 兼容问题。
// 非构建环境（如 e2e 测试的 mocha）下 define 不存在，
// 用 typeof 检查 + fallback 避免 ReferenceError。
declare const __MODE_VERSION__: string;
declare const __MODE_COMMIT__: string;
declare const __MODE_BUILD_TIME__: string;

export const MODE_VERSION: string =
  typeof __MODE_VERSION__ !== "undefined" ? __MODE_VERSION__ : "0.0.0-dev";
export const MODE_COMMIT: string =
  typeof __MODE_COMMIT__ !== "undefined" ? __MODE_COMMIT__ : "unknown";
export const MODE_BUILD_TIME: string =
  typeof __MODE_BUILD_TIME__ !== "undefined" ? __MODE_BUILD_TIME__ : "unknown";
