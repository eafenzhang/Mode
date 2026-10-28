/**
 * 兼容入口：实现在 `@mode/server/remote/lanRemoteAttach.js`。
 * 2026-10-28 起配对、令牌读取与 LAN 挂接由 desktop Host 与 Web/fpk 共用一份，
 * 避免 server 反向依赖 desktop。
 */
export * from "@mode/server/remote/lanRemoteAttach.js";
