import { hostname } from "node:os";
import {
  createLocalServices,
  getAppConfigDir,
  initializeDataRootNonInteractive,
  resolveDataRootActionFromEnv,
} from "@mode/services/node";
import { ICredentialService } from "@mode/services";
import { MODE_VERSION } from "@mode/shared";
import {
  materializeBundledModeBuiltinProviderConfig,
  readBundledModeBuiltinProviderConfig,
} from "./bundledModeBuiltinProviderConfig.js";
import { createHttpServer } from "./http.js";
import { createLanAccessHost } from "./lanAccess.js";

async function main(): Promise<void> {
  // 数据根必须先于任何路径写入完成初始化/合法化（materialize 会写 getAppConfigDir()）。
  await initializeDataRootNonInteractive({
    createdBy: "server",
    appVersion: MODE_VERSION,
    action: resolveDataRootActionFromEnv(),
  });
  const modeBuiltinProviderConfigFilePath = await materializeBundledModeBuiltinProviderConfig({
    environmentConfigRoot: getAppConfigDir(),
    content: readBundledModeBuiltinProviderConfig(),
  });
  const port = Number(process.env["PORT"]) || 3030;
  const host = process.env["MODE_SERVER_HOST"]?.trim() || process.env["HOST"]?.trim() || undefined;
  const staticRoot = process.env["MODE_WEB_STATIC_ROOT"]?.trim() || undefined;
  const authToken = process.env["MODE_SERVER_AUTH_TOKEN"]?.trim() || undefined;
  const services = createLocalServices({
    modeBuiltinProviderConfigFilePath,
    providerProvisioningTargetEnabled: Boolean(authToken),
  });

  // 设备展示名与 server-info 的 name 保持同一来源。
  const serverName = process.env["MODE_SERVER_NAME"]?.trim() || hostname() || "mode-server";
  // 局域网被连端（docs/specs/lan-host-server-mode.md）只在带访问令牌的部署下装配：
  // 无令牌（本地 dev）保持零鉴权现状，不引入额外中间件。
  const credentialService = services.getOptional(ICredentialService);
  const lanAccessHost =
    authToken && credentialService
      ? await createLanAccessHost({ credentials: credentialService, port, name: serverName })
      : undefined;

  createHttpServer(services, port, {
    ...(host ? { host } : {}),
    ...(staticRoot ? { staticRoot, spaFallback: true } : {}),
    ...(authToken
      ? {
          authToken,
          authRequired: true,
          // 静态访问令牌（浏览器/网关链路）与逐设备长期令牌并存：任一通过即鉴权成功；
          // serverId 由 lanAccessHost 提供，对端「我配对的对端」按它稳定归档。
          verifyAuthToken: (presented: string) =>
            presented === authToken || (lanAccessHost?.verifyToken(presented) ?? false),
        }
      : {}),
    ...(lanAccessHost
      ? {
          lanAccessHost,
          lanPairing: { redeem: (request) => lanAccessHost.redeem(request) },
          resolveLanClientByToken: (token) => lanAccessHost.resolveClientId(token),
          onLanClientWorkspace: (clientId, workspacePath) =>
            lanAccessHost.recordClientWorkspace(clientId, workspacePath),
          serverId: lanAccessHost.getServerId(),
          name: serverName,
        }
      : {}),
  });
}

void main().catch((error: unknown) => {
  console.error("[mode-server:http] startup failed", error);
  process.exitCode = 1;
});
