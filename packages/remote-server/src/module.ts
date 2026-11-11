/** remote-server 模块清单：远程控制 v3 的账号、设备目录、membership、Control 与 Relay。 */
export const remoteServerModule = {
  id: "remote-server",
  requires: ["remote-core"],
  provides: ["remote-account-service", "remote-connection-gateway"],
  publicEntrypoints: ["index.ts"],
} as const;
