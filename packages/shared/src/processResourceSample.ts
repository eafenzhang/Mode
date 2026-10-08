/** 进程资源样本的来源标识；兼容服务端资源查询协议，不包含事件上报。 */
export const PROCESS_RESOURCE_CLI_LANES = ["chat", "plugin", "mcp-status"] as const;
export type ProcessResourceCliLane = (typeof PROCESS_RESOURCE_CLI_LANES)[number];

/** CLI 进程资源采样周期；与 app 侧聚合共用同一节拍，避免两侧漂移。 */
export const MODE_CLI_RESOURCE_SAMPLE_INTERVAL_MS = 60_000;
