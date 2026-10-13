import { afterEach, describe, expect, it, vi } from "vitest";
import { resolveModeBuiltinProviderConfigFilePath } from "../src/main/desktopProviderConfig.js";

// 实现用 node:path join 产出本机分隔符（运行时语义正确），Windows 单测下需按
// POSIX 口径归一后再比较，期望值才能跨平台成立。
function toPosixPath(value: string) {
  return value.replaceAll("\\", "/");
}

describe("resolveModeBuiltinProviderConfigFilePath", () => {
  it("正式包读取 resources，开发态读取仓库 config", () => {
    expect(
      toPosixPath(
        resolveModeBuiltinProviderConfigFilePath({
          isPackaged: true,
          resourcesPath: "/Applications/Mode.app/Contents/Resources",
          appPath: "/Applications/Mode.app/Contents/Resources/app.asar",
          // 隔离宿主环境变量：Mode 开发运行时会注入 MODE_BUILTIN_PROVIDER_CONFIG_FILE，
          // 不显式清空会让本机跑单测时短路进 explicitPath 分支（CI 无此变量所以一直是绿的）。
          env: {},
        }),
      ),
    ).toBe("/Applications/Mode.app/Contents/Resources/config/provider/mode-builtin.json");

    expect(
      toPosixPath(
        resolveModeBuiltinProviderConfigFilePath({
          isPackaged: false,
          appPath: "/repo/packages/desktop",
          env: {},
        }),
      ),
    ).toBe("/repo/config/provider/mode-builtin.test.json");
  });

  it("显式 Mode Built-in Config Source 优先于安装布局", () => {
    expect(
      resolveModeBuiltinProviderConfigFilePath({
        env: { MODE_BUILTIN_PROVIDER_CONFIG_FILE: "/fixtures/mode-builtin.json" },
        isPackaged: true,
        resourcesPath: "/Applications/Mode.app/Contents/Resources",
      }),
    ).toBe("/fixtures/mode-builtin.json");
  });
});

describe("开发态按编译环境选取配置", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });
  it.each(["test", "production"])("%s 不受启动时的环境变量反向覆盖", async (environment) => {
    vi.resetModules();
    vi.stubGlobal("__MODE_ENV__", environment);
    const { resolveModeBuiltinProviderConfigFilePath: resolvePath } =
      await import("../src/main/desktopProviderConfig.js");
    expect(
      toPosixPath(
        resolvePath({
          isPackaged: false,
          appPath: "/repo/packages/desktop",
          env: { MODE_ENV: environment === "test" ? "production" : "test" },
        }),
      ),
    ).toBe(
      `/repo/config/provider/${environment === "production" ? "mode-builtin.json" : "mode-builtin.test.json"}`,
    );
  });
});
