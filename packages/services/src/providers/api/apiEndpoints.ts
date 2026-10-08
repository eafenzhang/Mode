import { buildRuntimeModeApiUrl, resolveZaiBusinessBaseUrl } from "@mode/shared";

export const MODE_CLIENT_SCENES_URL = buildRuntimeModeApiUrl(
  process.env,
  "/api/v1/client/scenes",
);

export const ZAI_API_HOST = resolveZaiBusinessBaseUrl(process.env);
