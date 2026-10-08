import type { ApiClient } from "@mode/shared";
import type { IClientScenesService } from "./clientScenes.js";

export function createClientScenesService(_dependencies: {
  apiClient: ApiClient;
}): IClientScenesService {
  // 审计版不连接官方服务；无远端场景时由客户端使用本地功能入口。
  return { list: async () => ({ code: 0, msg: "", data: [] }) };
}
