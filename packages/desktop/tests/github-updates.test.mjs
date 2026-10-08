import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { EventEmitter } from "node:events";
import { createRequire } from "node:module";
import { transpileModule, ModuleKind } from "typescript";

const require = createRequire(import.meta.url);
const feed = "https://github.com/eafenzhang/Mode/releases/latest/download/";
async function load(relative, imports = {}) {
  const source = await readFile(new URL(`../src/main/${relative}.ts`, import.meta.url), "utf8");
  const output = transpileModule(source, {
    compilerOptions: { module: ModuleKind.CommonJS },
  }).outputText;
  const exports = {};
  new Function("require", "exports", output)((name) => {
    assert.ok(name in imports, `unexpected runtime import: ${name}`);
    return imports[name];
  }, exports);
  return exports;
}
const logger = { info() {}, warn() {}, error() {}, debug() {} };

test("update modules have no official endpoint or custom manifest dependency", async () => {
  for (const name of ["autoUpdater", "forceUpdateGuard"]) {
    const source = await readFile(new URL(`../src/main/${name}.ts`, import.meta.url), "utf8");
    assert.doesNotMatch(
      source,
      /DEFAULT_MODE_ENDPOINT_ORIGIN|resolveRuntimeZCodeEndpointOrigin|manifestUpdateProvider|zcode\.z\.ai|\/api\/v1\/client\/configs/,
    );
  }
});

test("force guard never fetches, blocks or invokes callbacks", async () => {
  const guard = await load("forceUpdateGuard");
  const unexpected = () => assert.fail("force update must have no side effects");
  assert.deepEqual(
    await guard.maybeBlockStartupForForceUpdate({
      locale: "en-US",
      logger,
      fetchRemoteConfig: unexpected,
      requestAutoUpdate: unexpected,
      onBlocked: unexpected,
    }),
    { blocked: false },
  );
});

test("github default feed, generic overrides, manual check and native download/install remain wired", async () => {
  const updater = new EventEmitter();
  let checks = 0,
    downloads = 0,
    installs = 0,
    preparation = 0,
    configured;
  updater.setFeedURL = (value) => {
    configured = value;
  };
  updater.checkForUpdates = async () => {
    checks++;
    updater.emit("checking-for-update");
    updater.emit("update-not-available", { version: "1.0.0" });
  };
  updater.downloadUpdate = async () => {
    downloads++;
  };
  updater.quitAndInstall = () => {
    installs++;
  };
  const handlers = new Map();
  const sent = [];
  const win = {
    isDestroyed: () => false,
    webContents: { id: 1, send: (...args) => sent.push(args) },
  };
  const module = await load("autoUpdater", {
    electron: {
      app: { isPackaged: true, getVersion: () => "1.0.0" },
      BrowserWindow: { getAllWindows: () => [win], getFocusedWindow: () => win },
      Menu: { getApplicationMenu: () => null },
      ipcMain: { handle: (key, fn) => handlers.set(key, fn), on() {} },
    },
    "@mode/shared": {
      DEFAULT_LOCALE: "en-US",
      MODE_VERSION: "1.0.0",
      PlatformChannels: new Proxy({}, { get: (_, key) => key }),
      desktopMenuMessageIds: {},
      getDesktopMenuMessage: () => "",
      formatDesktopMenuMessage: () => "",
    },
    "electron-updater": {
      __esModule: true,
      default: { autoUpdater: updater },
      CancellationToken: class {
        dispose() {}
      },
    },
    semver: { __esModule: true, default: require("semver") },
    "./logger.js": { logger },
  });
  assert.deepEqual(
    module.resolveUpdateFeedSourceFromStartupConfig({
      argv: [],
      env: { MODE_UPDATE_FEED_URL: "https://example.invalid/feed/" },
    }),
    { url: "https://example.invalid/feed/" },
  );
  for (const argv of [
    ["--zcode-update-feed-url=https://example.invalid/cli/"],
    ["--zcode-update-feed-url", "https://example.invalid/cli/"],
  ]) {
    assert.deepEqual(
      module.resolveUpdateFeedSourceFromStartupConfig({
        argv,
        env: { MODE_UPDATE_FEED_URL: feed },
      }),
      { url: "https://example.invalid/cli/" },
    );
  }
  await module.initAutoUpdater({
    onBeforeQuitAndInstall: async () => {
      preparation++;
    },
  });
  await new Promise(setImmediate);
  // 默认更新源 = 本项目 GitHub 仓库的 Releases，走 GitHub provider 并允许预发布。
  assert.deepEqual(configured, {
    provider: "github",
    owner: "eafenzhang",
    repo: "Mode",
  });
  assert.equal(updater.allowPrerelease, true);
  assert.equal(checks, 1);
  module.refreshAutoUpdaterReleaseChannel(true);
  assert.equal(checks, 1);
  module.requestForceAutoUpdate(() => assert.fail("no callback"))();
  assert.equal(checks, 1);
  module.checkForUpdateMenuClick(win);
  await new Promise(setImmediate);
  assert.equal(checks, 2);
  updater.emit("update-available", { version: "2.0.0" });
  await new Promise(setImmediate);
  assert.equal(module.getAutoUpdaterState().kind, "update-available");
  await handlers.get("DownloadUpdate")();
  assert.equal(downloads, 1);
  updater.emit("update-downloaded", { version: "2.0.0" });
  await new Promise(setImmediate);
  assert.equal(module.getAutoUpdaterState().kind, "update-downloaded");
  await handlers.get("QuitAndInstallUpdate")();
  assert.equal(preparation, 1);
  assert.equal(installs, 1);
  assert.ok(sent.length > 0);
  await module.initAutoUpdater({ updateFeedSource: { url: "https://example.invalid/custom/" } });
  // 显式覆盖仍走 generic provider（测试/开发用）。
  assert.equal(configured.provider, "generic");
  assert.equal(configured.url, "https://example.invalid/custom/");
  assert.equal(configured.channel, "latest");
  await module.initAutoUpdater({ enabled: false });
});

test("built-in provider reads platform YAML and resolves release assets", async () => {
  const { GenericProvider } = require("electron-updater/out/providers/GenericProvider.js");
  for (const [platform, suffix] of [
    ["darwin", "-mac"],
    ["win32", ""],
    ["linux", `-linux${process.arch === "x64" ? "" : `-${process.arch}`}`],
  ]) {
    let requested;
    const provider = new GenericProvider(
      { provider: "generic", url: feed, channel: "latest" },
      { isAddNoCacheQuery: false },
      {
        platform,
        executor: {
          request: async (options) => {
            requested = `https://${options.hostname}${options.path}`;
            return "version: 2.0.0\nfiles:\n  - url: app.zip\n    sha512: test-checksum\n";
          },
        },
      },
    );
    const info = await provider.getLatestVersion();
    assert.equal(requested, `${feed}latest${suffix}.yml`);
    assert.equal(provider.resolveFiles(info)[0].url.href, `${feed}app.zip`);
  }
});
