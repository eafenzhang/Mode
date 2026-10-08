import {
  modeProtocolMethods,
  modePluginsReferenceCatalogResultSchema,
  type ModePluginsReferenceCatalogParams,
} from "@mode/shared";
import type { ModeProtocolClient } from "#src/mode-agent/modeProtocolClient.js";

/** 旧协议严格校验响应；新展示字段走独立入口，只有 -32601 能证明旧 Agent 不支持。 */
export async function requestPluginReferenceCatalog(
  client: Pick<ModeProtocolClient, "request">,
  params: ModePluginsReferenceCatalogParams,
) {
  try {
    return await client.request(
      modeProtocolMethods.pluginsReferenceCatalogWithCategory,
      params,
      modePluginsReferenceCatalogResultSchema,
    );
  } catch (error) {
    if (!(typeof error === "object" && error !== null && "code" in error && error.code === -32601))
      throw error;
    return client.request(
      modeProtocolMethods.pluginsReferenceCatalog,
      params,
      modePluginsReferenceCatalogResultSchema,
    );
  }
}
