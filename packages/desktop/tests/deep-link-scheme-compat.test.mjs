import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { transpileModule, ModuleKind } from "typescript";

// deep link 改名（zcode:// → mode://）后的兼容：只注册 mode，但依旧处理 zcode:// 传入链接。
// desktopDeepLinkUrl.ts 不依赖 electron，用 transpile + new Function 直接加载真实源码。
const root = new URL("../../../", import.meta.url);

async function loadDeepLinkModule() {
  const source = await readFile(
    new URL("packages/desktop/src/main/desktopDeepLinkUrl.ts", root),
    "utf8",
  );
  const exports = {};
  new Function(
    "require",
    "exports",
    transpileModule(source, {
      compilerOptions: { module: ModuleKind.CommonJS },
    }).outputText,
  )((name) => {
    throw new Error(`desktopDeepLinkUrl.ts 不应有 import（出现 ${name}）`);
  }, exports);
  return exports;
}

const deepLink = await loadDeepLinkModule();

test("OAuth 回调：mode:// 与改名前的 zcode:// 都被识别", () => {
  assert.equal(deepLink.isOAuthCallbackUrl(new URL("mode://oauth/callback")), true);
  assert.equal(deepLink.isOAuthCallbackUrl(new URL("zcode://oauth/callback")), true);
  assert.equal(deepLink.isOAuthCallbackUrl(new URL("https://oauth/callback")), false);
});

test("工作区打开：旧 scheme 链接仍可解析 path", () => {
  const legacy = new URL("zcode://workspace/open?path=C%3A%5Cwork%5Cdemo");
  assert.equal(deepLink.isWorkspaceOpenUrl(legacy), true);
  assert.equal(deepLink.extractWorkspaceOpenPath(legacy), "C:\\work\\demo");
});

test("share import：旧 scheme 链接仍可解析 code", () => {
  const legacy = new URL("zcode://share/import?code=abc123");
  assert.equal(deepLink.isShareImportUrl(legacy), true);
  assert.equal(deepLink.extractShareImportCode(legacy), "abc123");
});

test("argv 提取：两种 scheme 都能从命令行取出完整链接", () => {
  const modeUrl = deepLink.extractDeepLinkUrlFromArgs([
    "C:\\Program Files\\Mode\\Mode.exe",
    "mode://oauth/callback?state=state-1",
  ]);
  assert.equal(modeUrl, "mode://oauth/callback?state=state-1");

  const legacyUrl = deepLink.extractDeepLinkUrlFromArgs([
    "C:\\Program Files\\Mode\\Mode.exe",
    "zcode://oauth/callback?state=state-2",
  ]);
  assert.equal(legacyUrl, "zcode://oauth/callback?state=state-2");

  // 非 deep link 参数（普通路径、URL 前缀在词中间）不应被当成链接。
  assert.equal(deepLink.extractDeepLinkUrlFromArgs(["C:\\work\\asmode.txt"]), null);
});
