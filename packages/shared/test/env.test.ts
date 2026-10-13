import { afterEach, describe, expect, it, vi } from "vitest";

async function importFreshEnvModule() {
  vi.resetModules();
  return await import("../src/env.js");
}

describe("shared runtime env", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("构建期可选能力未注入时关闭，事件上报端点取测试注入值", async () => {
    const env = await importFreshEnvModule();

    // 单测不注入 ARMS 与自动更新；事件上报端点由 vitest.config.ts 固定注入。
    expect(env.MODE_ARMS_RUM_ENDPOINT).toBe("");
    expect(env.MODE_AUTO_UPDATE_ENABLED).toBe(false);
    expect(env.MODE_TELEMETRY_REPORT_ENDPOINT).toBe("https://mode.z.ai/api/v1/event/report");
  });

  it("未注入 MODE_ENV 时默认 test 并映射为 local", async () => {
    vi.stubGlobal("__MODE_ENV__", undefined);

    const env = await importFreshEnvModule();

    expect(env.MODE_ENV).toBe("test");
    expect(env.normalizeModeEnv("development")).toBe("test");
    expect(env.mapModeEnvToArmsRumEnv("production")).toBe("local");
  });

  it("未注入身份 define 时 flavor 跟随 MODE_ENV", async () => {
    vi.stubGlobal("__MODE_ENV__", "production");
    vi.stubGlobal("__MODE_PRODUCT_FLAVOR__", undefined);

    const env = await importFreshEnvModule();

    expect(env.MODE_PRODUCT_FLAVOR).toBe("production");
    expect(env.normalizeModeProductFlavor(undefined, "test")).toBe("preview");
    expect(env.normalizeModeProductFlavor("bogus", "production")).toBe("production");
  });

  it("显式注入 preview 身份时生产后端构建仍是 Preview", async () => {
    vi.stubGlobal("__MODE_ENV__", "production");
    vi.stubGlobal("__MODE_PRODUCT_FLAVOR__", "preview");

    const env = await importFreshEnvModule();

    expect(env.MODE_ENV).toBe("production");
    expect(env.MODE_PRODUCT_FLAVOR).toBe("preview");
  });

  it("mapModeEnvToArmsRumEnv 将非本地 production 映射为 prod", async () => {
    vi.stubGlobal("__MODE_ENV__", "production");

    const env = await importFreshEnvModule();

    expect(env.mapModeEnvToArmsRumEnv("production")).toBe("prod");
  });

  it("mapModeEnvToArmsRumEnv 在本地开发态忽略 production 产品环境并映射为 local", async () => {
    vi.stubGlobal("__MODE_ENV__", "production");

    const env = await importFreshEnvModule();

    expect(env.mapModeEnvToArmsRumEnv("development")).toBe("local");
  });
});
