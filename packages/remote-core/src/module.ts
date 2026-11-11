/** remote-core 模块清单：远程控制 v3 的协议、设备身份、Noise 安全通道与连接编排。 */
export const remoteCoreModule = {
  id: "remote-core",
  requires: [],
  provides: ["remote-protocol", "remote-secure-session", "remote-runtime"],
  publicEntrypoints: ["contract.ts", "index.ts"],
} as const;
