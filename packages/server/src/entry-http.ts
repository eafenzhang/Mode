import {
  createLocalServices,
  getAppConfigDir,
  initializeDataRootNonInteractive,
  resolveDataRootActionFromEnv,
} from "@mode/services/node";
import { MODE_VERSION } from "@mode/shared";
import {
  materializeBundledModeBuiltinProviderConfig,
  readBundledModeBuiltinProviderConfig,
} from "./bundledModeBuiltinProviderConfig.js";
import { createHttpServer } from "./http.js";

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

  createHttpServer(services, port, {
    ...(host ? { host } : {}),
    ...(staticRoot ? { staticRoot, spaFallback: true } : {}),
    ...(authToken ? { authToken, authRequired: true } : {}),
  });
}

void main().catch((error: unknown) => {
  console.error("[mode-server:http] startup failed", error);
  process.exitCode = 1;
});
