import { assertOfficialPlatformAccessible } from "@mode/shared";
import type { EnvRecord } from "./model-execution.js";

export interface OfficialCodingPlanGatewayRoute {
  readonly providerEndpoint: string;
  readonly gatewayPath: string;
}

// 审计版不连接官方服务：用户选择的模型端点不再改写到平台网关。
export const OFFICIAL_CODING_PLAN_GATEWAY_ROUTES: readonly OfficialCodingPlanGatewayRoute[] = [];

export interface OfficialCodingPlanGatewayDecision {
  readonly viaGateway: boolean;
  readonly url: string;
}
export type OfficialCodingPlanGatewayFetch = typeof globalThis.fetch;

export function resolveOfficialCodingPlanGatewayUrl(
  requestUrl: string,
  _env: EnvRecord = process.env,
): OfficialCodingPlanGatewayDecision {
  assertOfficialPlatformAccessible(requestUrl);
  return { viaGateway: false, url: requestUrl };
}

export function createOfficialCodingPlanGatewayFetch(options: {
  env?: EnvRecord;
  fetch: OfficialCodingPlanGatewayFetch;
}): OfficialCodingPlanGatewayFetch {
  return async (input, init) => {
    // 历史内置配置可能仍指向平台网关；请求前拒绝，保留用户模型的原始参数。
    assertOfficialPlatformAccessible(input instanceof Request ? input.url : input);
    return options.fetch(input, init);
  };
}
