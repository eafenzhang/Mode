/**
 * OrcaRouter 服务契约（descriptor + 类型）。
 *
 * 与 `service.ts` 拆开的原因：实现依赖 `node:crypto`（PKCE/HMAC），而 UI 的 barrel 只
 * 需要 descriptor。浏览器 dev/运行时若从 root barrel 拿到实现模块，会把 node:crypto
 * 带进页面（生产构建靠 tree-shaking 才没暴露）。
 */
import type { OrcaCapability, OrcaCredentialSource } from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";
import type { OrcaCatalogResult } from "./catalog.js";
import type { OrcaConnectError, OrcaConnectState } from "./connect.js";
import type { OrcaCredentialStatus } from "./credentialStore.js";

/** 对外的目录条目最小元数据（不包含任何凭据） */
export interface OrcaRouterModelOption {
  readonly modelId: string;
  readonly inputModalities: readonly string[];
  readonly supportedEndpointTypes: readonly string[];
}

export interface OrcaRouterCatalogView {
  readonly capability: OrcaCapability;
  readonly source: OrcaCatalogResult["source"];
  readonly degraded: boolean;
  readonly fetchedAt: number;
  readonly totalBeforeFilter: number;
  readonly error?: string;
  readonly models: readonly OrcaRouterModelOption[];
}

/** 两个 origin 的公开投影；用于证书与测试断言，不含密钥 */
export interface OrcaRouterEndpoints {
  readonly authBase: string;
  readonly apiBase: string;
  readonly inferenceBase: string;
}

/** 下游真正使用的凭据形状；两种 adapter 产出完全相同的结构 */
export interface OrcaRouterResolvedCredential {
  readonly masked: string;
  readonly source: OrcaCredentialSource;
  readonly generation: number;
}

export type OrcaRouterConnectResult =
  | { readonly ok: true; readonly state: OrcaConnectState }
  | {
      readonly ok: false;
      readonly kind: OrcaConnectError["kind"];
      readonly message: string;
      readonly state: OrcaConnectState;
    };

export interface IOrcaRouterService {
  getEndpoints(): Promise<OrcaRouterEndpoints>;
  getCredentialStatus(): Promise<OrcaCredentialStatus>;
  /** API Key adapter：保存或更新手填密钥 */
  saveApiKey(input: { readonly apiKey: string }): Promise<OrcaCredentialStatus>;
  /** API Key adapter：清除 */
  clearCredential(): Promise<OrcaCredentialStatus>;
  /** 证明两条入口产出同一种凭据结果（下游不关心来源） */
  resolveCredential(input: {
    readonly source: OrcaCredentialSource;
  }): Promise<OrcaRouterResolvedCredential | null>;
  /** PKCE adapter：开始登录，返回用户需要打开的授权 URL */
  beginConnect(): Promise<OrcaConnectState>;
  /** PKCE adapter：提交用户粘贴的授权码 */
  submitConnectCode(input: { readonly code: string }): Promise<OrcaRouterConnectResult>;
  cancelConnect(input?: { readonly reason?: string }): Promise<OrcaConnectState>;
  /** 卸载 / pagehide / 切换认证方式时同步清理登录锁 */
  invalidateConnect(input: {
    readonly reason: "pagehide" | "unmount" | "provider-switch" | "auth-method-switch";
  }): Promise<OrcaConnectState>;
  listModels(input: {
    readonly capability: OrcaCapability;
    readonly requiredInputModality?: "image" | "audio" | "video";
    readonly forceRefresh?: boolean;
  }): Promise<OrcaRouterCatalogView>;
  /** 上游 401：只标记发出被拒请求的精确账号与 generation */
  reportUnauthorized(input: {
    readonly generation?: number;
    readonly accountId?: string;
  }): Promise<{ readonly marked: boolean; readonly status: OrcaCredentialStatus }>;
  /**
   * 把当前 store 凭据对齐到 OrcaRouter provider 的推理配置。
   *
   * 保存/换取/清除/401 都会自动对齐；本方法是启动期的显式兜底，
   * 也是自动化测试验证「凭据确实到达推理路径」的入口。
   */
  reconcileProviderCredential(): Promise<OrcaCredentialStatus>;
}

export const IOrcaRouterService = createServiceDescriptor<IOrcaRouterService>(
  ServiceChannels.OrcaRouter,
);
