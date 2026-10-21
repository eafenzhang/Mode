import { existsSync } from "node:fs";
import { join } from "node:path";

/**
 * 桌面 LAN 服务挂载的 Web 静态资源目录。
 *
 * 优先级：显式 `MODE_WEB_STATIC_ROOT` > 打包资源 `<MODE_HOST_RESOURCES_DIR>/web` >
 * 开发态 `packages/web/dist`。没有产物时返回 undefined，LAN 服务保持 API-only。
 */
export function resolveWebStaticRoot(env: NodeJS.ProcessEnv = process.env): string | undefined {
  const explicit = env.MODE_WEB_STATIC_ROOT?.trim();
  if (explicit && existsSync(explicit)) {
    return explicit;
  }
  const resourcesDir = env.MODE_HOST_RESOURCES_DIR?.trim();
  if (resourcesDir) {
    const packaged = join(resourcesDir, "web");
    if (existsSync(packaged)) {
      return packaged;
    }
  }
  const dev = join(process.cwd(), "packages", "web", "dist");
  return existsSync(dev) ? dev : undefined;
}
