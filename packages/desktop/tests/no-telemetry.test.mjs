import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
const root = new URL("../", import.meta.url);
async function sources(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  return (
    await Promise.all(
      entries.map(async (entry) => {
        const url = new URL(entry.name + (entry.isDirectory() ? "/" : ""), dir);
        return entry.isDirectory() ? sources(url) : /\.(ts|tsx|js)$/.test(entry.name) ? [url] : [];
      }),
    )
  ).flat();
}
test("desktop cannot initialize telemetry exporters or forwarding bridges", async () => {
  for (const file of await sources(new URL("src/", root))) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(
      content,
      /@arms\/rum-electron|@opentelemetry\/|createTelemetryCore|createDesktopTelemetryFetch|installArmsRumBridgeIpcForward|new NetworkTelemetryChannelServer|registerHostNetworkTelemetry/,
      file.pathname,
    );
    assert.doesNotMatch(
      content,
      /ipcRenderer\.(?:invoke|send)\(PlatformChannels\.(?:ReportTelemetryEvent|ReportArmsCustomEvent|ReportLocalTtftBatch|ReportRendererActionTraceBatch|SyncTelemetryContext)/,
      file.pathname,
    );
  }
  const manifest = JSON.parse(await readFile(new URL("package.json", root), "utf8"));
  assert.equal(
    Object.keys(manifest.dependencies).some((name) => /^@(?:arms|opentelemetry)\//.test(name)),
    false,
  );
});

// 以 Electron 边界替身执行真实模块，检查删除旁路后业务事件仍能流转。
async function loadModule(relative, imports) {
  const { transpileModule, ModuleKind } = await import("typescript");
  const source = await readFile(new URL(relative, root), "utf8");
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

test("Host environment discards exporter settings while retaining business configuration", async () => {
  const { omitDesktopTelemetryEnvironment } = await loadModule(
    "src/main/desktopTelemetryPolicy.ts",
    {},
  );
  const input = {
    OTEL_EXPORTER_OTLP_ENDPOINT: "https://example.invalid",
    MODE_TELEMETRY_DEVICE_MID: "test",
    MODE_MODEL_TELEMETRY_ENABLED: "true",
    HTTPS_PROXY: "http://localhost:8080",
    PATH: "/test/bin",
  };
  assert.deepEqual(omitDesktopTelemetryEnvironment(input), {
    HTTPS_PROXY: input.HTTPS_PROXY,
    PATH: input.PATH,
  });
  assert.equal(input.MODE_MODEL_TELEMETRY_ENABLED, "true");
});

test("database startup still forwards ordered state, starts ready listeners and accepts controls", async () => {
  const { EventEmitter } = await import("node:events");
  const ipcMain = new EventEmitter();
  const win = new EventEmitter();
  const child = new EventEmitter();
  const forwarded = [],
    controls = [];
  win.isDestroyed = () => false;
  win.webContents = { isDestroyed: () => false, send: (_channel, state) => forwarded.push(state) };
  child.postMessage = (message) => controls.push(message);
  const relay = await loadModule("src/main/databaseStartupRelay.ts", {
    "node:crypto": await import("node:crypto"),
    electron: { ipcMain },
    "@zcode/shared": {
      HostMessageTypes: { DatabaseStartupControl: "control" },
      InternalChannels: { DatabaseStartupState: "state", DatabaseStartupControl: "control" },
      databaseStartupControlSchema: { safeParse: (data) => ({ success: true, data }) },
    },
  });
  let ready = 0;
  relay.onLocalDatabaseStartupReady(() => ready++);
  const binding = relay.bindDatabaseStartupRelay(win, child, "startup");
  binding.receive({ startupId: "stale", sequence: 5, phase: "ready" });
  binding.receive({ startupId: "startup", sequence: 1, phase: "ready" });
  binding.receive({ startupId: "startup", sequence: 1, phase: "ready" });
  assert.equal(ready, 1);
  assert.equal(forwarded.length, 1);
  ipcMain.emit("control", { sender: win.webContents }, { action: "snapshot" });
  assert.equal(forwarded.length, 2);
  assert.equal(controls.length, 1);
  child.emit("exit");
  assert.equal(forwarded.at(-1).phase, "failed");
  win.emit("closed");
  assert.equal(ipcMain.listenerCount("control"), 0);
});

test("OAuth delivery and remote connection retain business behavior without telemetry handlers", async () => {
  const handlers = new Map();
  const channels = new Proxy({}, { get: (_target, key) => key });
  let delivered = 0;
  const win = {};
  const { registerRemoteIpcHandlers } = await loadModule("src/main/desktopMainIpcRemote.ts", {
    electron: {
      app: { on() {} },
      BrowserWindow: { fromWebContents: () => win },
      ipcMain: {
        on: (name, cb) => handlers.set(name, cb),
        handle: (name, cb) => handlers.set(name, cb),
      },
    },
    "@zcode/shared": {
      PlatformChannels: channels,
      InternalChannels: channels,
      remoteTargetSchema: { safeParse: (data) => ({ success: true, data }) },
      normalizeUnknownError: (error) => ({ message: error.message }),
    },
    "./desktopNotifications.js": {},
    "./desktopOAuthDeepLink.js": { deliverPendingDeepLink: () => delivered++ },
    "./desktopMainIpcHelpers.js": {},
  });
  let args;
  registerRemoteIpcHandlers({
    logger: { warn() {}, error() {} },
    createRemoteWorkspaceSession: async (...input) => {
      args = input;
      return "session";
    },
  });
  handlers.get("RendererReady")({ sender: {} });
  assert.equal(delivered, 1);
  const target = { kind: "ssh", host: "example.invalid" };
  assert.deepEqual(
    await handlers.get("ConnectRemote")(
      { sender: {} },
      { target, requestId: "request", workspacePath: "/project", workspaceIdentity: "identity" },
    ),
    { success: true, sessionId: "session" },
  );
  assert.deepEqual(args, [
    win,
    target,
    "request",
    { workspacePath: "/project", workspaceIdentity: "identity" },
  ]);
  for (const name of handlers.keys()) assert.doesNotMatch(name, /Telemetry|Arms|Trace|Ttft/);
});
