export type ModeEnv = "test" | "production";
/** 安装包身份：决定应用名、app id、Electron 数据目录与更新策略；与后端环境 `ModeEnv` 是两个轴。 */
export type ModeProductFlavor = "production" | "preview";

// 非构建环境（如 e2e 测试的 mocha）下 define 不存在，用 typeof 检查 + fallback 避免 ReferenceError
declare const __MODE_ENV__: string;
declare const __MODE_PRODUCT_FLAVOR__: string;

export function normalizeModeEnv(value: string | undefined): ModeEnv {
  return value?.trim().toLowerCase() === "production" ? "production" : "test";
}

export const MODE_ENV = normalizeModeEnv(
  typeof __MODE_ENV__ !== "undefined" ? __MODE_ENV__ : undefined,
);

/**
 * 身份缺省跟随后端环境（test → preview，production → production）。
 * 桌面构建通过 `MODE_PREVIEW_IDENTITY=1` 显式注入 preview，得到连接生产后端的 Preview 包；
 * 未注入 define 的 bundle（web、CLI、测试）沿用旧的单轴语义。
 */
export function normalizeModeProductFlavor(
  value: string | undefined,
  modeEnv: ModeEnv,
): ModeProductFlavor {
  const normalized = value?.trim().toLowerCase();
  if (normalized === "production" || normalized === "preview") {
    return normalized;
  }
  return modeEnv === "production" ? "production" : "preview";
}

export const MODE_PRODUCT_FLAVOR = normalizeModeProductFlavor(
  typeof __MODE_PRODUCT_FLAVOR__ !== "undefined" ? __MODE_PRODUCT_FLAVOR__ : undefined,
  MODE_ENV,
);
export const MODE_APP_VERSION_ENV = "MODE_APP_VERSION" as const;
export const MODE_BUILD_COMMIT_ID_ENV = "MODE_BUILD_COMMIT_ID" as const;

// ── 运行时环境变量（不经过编译打包，启动时从 process.env 读取） ──
// 启用调试模式，值为 inspect-brk 的端口号，如 MODE_DEBUG=9230
export const RUNTIME_MODE_DEBUG =
  typeof process !== "undefined" ? process.env.MODE_DEBUG : undefined;
