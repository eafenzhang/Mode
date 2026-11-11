import {
  buildOrcaV1Base,
  type OrcaCapability,
  type OrcaOrigins,
} from "@mode/shared";
import { createServiceLogger } from "../logger/serviceLogger.js";
import { createOrcaCatalogService, type OrcaCatalogResult } from "./catalog.js";
import { OrcaConnectController, OrcaConnectError } from "./connect.js";
import { validateOrcaApiKeyInput, type OrcaCredentialAdapters } from "./credentials.js";
import type { OrcaCredentialStore } from "./credentialStore.js";
import type { OrcaProviderCredentialBinding } from "./providerOverlay.js";
import type {
  IOrcaRouterService,
  OrcaRouterCatalogView,
  OrcaRouterConnectResult,
  OrcaRouterEndpoints,
  OrcaRouterModelOption,
  OrcaRouterResolvedCredential,
} from "./descriptor.js";

export type {
  OrcaRouterCatalogView,
  OrcaRouterConnectResult,
  OrcaRouterEndpoints,
  OrcaRouterModelOption,
  OrcaRouterResolvedCredential,
} from "./descriptor.js";
export { IOrcaRouterService } from "./descriptor.js";

/** 只保留错误消息文本；绝不让可能的密钥片段进入日志。 */
function describeError(error: unknown): string {
  return error instanceof Error ? error.message : "未知错误";
}

export interface CreateOrcaRouterServiceInput {
  readonly store: OrcaCredentialStore;
  readonly adapters: OrcaCredentialAdapters;
  readonly connect: OrcaConnectController;
  readonly origins: OrcaOrigins;
  readonly catalog?: ReturnType<typeof createOrcaCatalogService>;
  /**
   * 把 store 凭据投递到 provider 推理配置的 seam。
   *
   * 推理路径只读 provider 自身的 `access.apiKey`：保存、PKCE 换取、清除与 401 标记后
   * 都必须把同一把 key（或 null）同步进目标 provider 的 Personal Overlay，
   * 否则会出现「设置页已连接、聊天请求却不带 key」的断链。
   */
  readonly credentialBinding?: OrcaProviderCredentialBinding;
}

export function createOrcaRouterService(input: CreateOrcaRouterServiceInput): IOrcaRouterService {
  const logger = createServiceLogger("orcarouter");
  const log = (level: "info" | "warn", message: string) => logger[level](message);
  const connect =
    input.connect ??
    new OrcaConnectController({
      credentialStore: input.store,
      origins: input.origins,
      appName: "Mode",
      log,
    });
  const catalog =
    input.catalog ??
    createOrcaCatalogService({
      credentialStore: input.store,
      origins: input.origins,
      log,
    });

  const endpoints: OrcaRouterEndpoints = Object.freeze({
    authBase: input.origins.authBase,
    apiBase: input.origins.apiBase,
    inferenceBase: buildOrcaV1Base(input.origins.apiBase),
  });

  // 凭据变更后必须立刻对齐 provider 推理配置；失败只告警，不反转已确认的凭据事实。
  const syncCredentialToProvider = async (): Promise<void> => {
    try {
      await input.credentialBinding?.sync();
    } catch (error) {
      log("warn", `OrcaRouter 凭据未能写入 Provider 推理配置：${describeError(error)}`);
    }
  };

  const toView = (capability: OrcaCapability, result: OrcaCatalogResult): OrcaRouterCatalogView =>
    Object.freeze({
      capability,
      source: result.source,
      degraded: result.degraded,
      fetchedAt: result.fetchedAt,
      totalBeforeFilter: result.totalBeforeFilter,
      ...(result.error ? { error: result.error } : {}),
      models: Object.freeze(
        result.models.map((model) =>
          Object.freeze({
            modelId: model.id,
            inputModalities: model.inputModalities,
            supportedEndpointTypes: model.supportedEndpointTypes,
          }),
        ),
      ),
    });

  return {
    async getEndpoints() {
      return endpoints;
    },

    getCredentialStatus() {
      return input.store.status();
    },

    async saveApiKey({ apiKey }) {
      const validated = validateOrcaApiKeyInput(apiKey);
      if (!validated) {
        // 空值等价于清除；不保留旧密钥。
        await input.store.clear();
        catalog.invalidate();
        await syncCredentialToProvider();
        return input.store.status();
      }
      await input.store.save({ apiKey: validated, source: "api-key" });
      catalog.invalidate();
      // 推理路径只读 provider 的 access.apiKey：手填 Key 必须写回同一把。
      await syncCredentialToProvider();
      log("info", "OrcaRouter API Key 已保存");
      return input.store.status();
    },

    async clearCredential() {
      // 显式断开：只有用户主动清除时才删除，不在登录失败时静默删除旧密钥。
      connect.cancel("用户清除凭据");
      await input.store.clear();
      catalog.invalidate();
      await syncCredentialToProvider();
      log("info", "OrcaRouter 凭据已清除");
      return input.store.status();
    },

    async resolveCredential({ source }) {
      const adapter = source === "pkce" ? input.adapters.pkce : input.adapters.apiKey;
      // adapter.acquire() 是唯一凭据入口；这里只确认它确实产出了可用凭据，
      // 返回值保持脱敏形状，真实密钥不越过 host 边界。
      await adapter.acquire();
      const status = await input.store.status();
      return Object.freeze({
        masked: status.masked,
        source: adapter.source,
        generation: status.generation,
      });
    },

    async beginConnect() {
      // 切换认证方式 / 重新发起前先失效旧尝试，避免两个登录互相覆盖。
      connect.cancel("重新发起授权");
      const state = connect.begin();
      log("info", "OrcaRouter 授权已开始");
      return state;
    },

    async submitConnectCode({ code }) {
      try {
        await connect.submitCode(code);
        catalog.invalidate();
        // PKCE 换回的长期 key 与手填路径落到同一处：写回 provider 推理配置。
        await syncCredentialToProvider();
        return { ok: true, state: connect.getState() };
      } catch (error) {
        if (error instanceof OrcaConnectError) {
          return { ok: false, kind: error.kind, message: error.message, state: connect.getState() };
        }
        log("warn", "OrcaRouter 授权码换取失败");
        return {
          ok: false,
          kind: "network",
          message: "OrcaRouter 授权码换取失败",
          state: connect.getState(),
        };
      }
    },

    async cancelConnect(opts) {
      return connect.cancel(opts?.reason ?? "用户取消");
    },

    async invalidateConnect({ reason }) {
      const state = connect.invalidateForPageHide();
      log("info", `OrcaRouter 登录状态已清理（${reason}）`);
      return state;
    },

    async listModels({ capability, requiredInputModality, forceRefresh }) {
      const result = await catalog.list({
        capability,
        ...(requiredInputModality ? { requiredInputModality } : {}),
        ...(forceRefresh ? { forceRefresh } : {}),
      });
      return toView(capability, result);
    },

    async reportUnauthorized({ generation, accountId }) {
      const marked = await input.store.markNeedsReauth({
        ...(generation !== undefined ? { generation } : {}),
        ...(accountId !== undefined ? { accountId } : {}),
      });
      if (marked) {
        // 长期 API key 被撤销后必须重新认证；这里不伪造 refresh，也不主动删除旧密钥。
        // 但必须立刻让推理路径停止使用这把死 key：loadUsable 已返回 null，同步即清除 provider 上的 apiKey。
        await syncCredentialToProvider();
        log("warn", "OrcaRouter 凭据被上游拒绝，已标记为需要重新认证");
      }
      return { marked, status: await input.store.status() };
    },

    async reconcileProviderCredential() {
      await syncCredentialToProvider();
      return input.store.status();
    },
  };
}
