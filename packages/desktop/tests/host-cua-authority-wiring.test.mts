/**
 * 装配守卫：Tauri Host 必须声明自己是 `desktop-local` authority。
 *
 * 背景（2026-10-26 远程桌面排查）：CUA Helper 的创建准入
 * （`shouldCreateDefaultCuaProductHelper`）要求 `serviceAuthorityMode === "desktop-local"`
 * **严格相等**。旧 Electron main 装配过这个值；Tauri Host 移植时漏传，导致被控端
 * `rd_media_start` 固定失败（`Computer Use Helper is not reachable for rd_media`），
 * 主控只看到「连不上」。
 *
 * 单元层已有准入用例（`packages/services/test/cua-helper-admission.test.ts`），本守卫补的是
 * **装配层**：确保 Host 真的把值传下去。静态扫描足够——这里要守的是「有没有这一行」。
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("Host 装配把 serviceAuthorityMode: desktop-local 传给 createLocalServices", () => {
  // 以本文件位置解析（CI 从仓库根跑、包内跑都成立，不依赖 cwd）。
  const source = readFileSync(
    fileURLToPath(new URL("../src/host-standalone/serviceAccessor.ts", import.meta.url)),
    "utf8",
  );
  assert.ok(
    /createLocalServices\(\{[\s\S]*?serviceAuthorityMode:\s*"desktop-local"/.test(source),
    'serviceAccessor.ts 的 createLocalServices 选项必须含 serviceAuthorityMode: "desktop-local"；' +
      "漏传会让 CUA Helper 永不创建（远程桌面媒体/输入全部 fail-closed）",
  );
});
