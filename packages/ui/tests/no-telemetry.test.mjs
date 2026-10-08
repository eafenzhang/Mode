import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../../", import.meta.url));
async function sources(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((e) =>
      e.isDirectory()
        ? sources(`${dir}/${e.name}`)
        : /\.(ts|tsx)$/.test(e.name) && !e.name.endsWith(".d.ts")
          ? [`${dir}/${e.name}`]
          : [],
    ),
  );
  return nested.flat();
}
test("UI and platform adapters have no telemetry collection or reporting entrypoints", async () => {
  for (const dir of ["packages/ui/src", "packages/web/src", "packages/desktop/src/renderer"]) {
    for (const file of await sources(root + dir)) {
      assert.doesNotMatch(
        await readFile(file, "utf8"),
        /reportTelemetryEvent|reportArmsCustomEvent|reportAppTelemetryEvent|runUserAction|startUserAction|ConversationTelemetrySupervisor|\/telemetry\//,
        file,
      );
    }
  }
});
test("platform and server do not expose or initialize reporting", async () => {
  assert.doesNotMatch(
    await readFile(root + "packages/shared/src/platform.ts", "utf8"),
    /reportTelemetryEvent|reportArmsCustomEvent|RendererActionTrace/,
  );
  for (const dir of ["packages/server/src", "packages/zcode-server-cli/src"]) {
    for (const file of await sources(root + dir))
      assert.doesNotMatch(await readFile(file, "utf8"), /processResourceTelemetry\s*:/, file);
  }
});

async function loadWebviewHelpers() {
  const { transpileModule, ModuleKind } = await import("typescript");
  const source = await readFile(
    root + "packages/ui/src/settings/model-provider-section/codingPlanEmbeddedWebview.ts",
    "utf8",
  );
  const output = transpileModule(source, {
    compilerOptions: { module: ModuleKind.CommonJS },
  }).outputText;
  const exports = {};
  new Function("require", "exports", output)((name) => {
    assert.equal(name, "@mode/shared");
    return {};
  }, exports);
  return exports;
}

test("purchase WebView retains authentication, theme and locale without injecting tracking context", async () => {
  const { runInNewContext } = await import("node:vm");
  const helpers = await loadWebviewHelpers();
  for (const provider of ["zai", "bigmodel"]) {
    const values = new Map([["zcode:coding-plan:report-context", "legacy context"]]);
    const events = [];
    const classes = new Map();
    const host = {
      localStorage: { setItem: (k, v) => values.set(k, v), removeItem: (k) => values.delete(k) },
      window: { __zcodeReportContext__: {}, dispatchEvent: (e) => events.push(e) },
      document: { documentElement: { classList: { toggle: (k, v) => classes.set(k, v) } } },
      CustomEvent: class {
        constructor(type, options) {
          this.type = type;
          this.detail = options.detail;
        }
      },
    };
    runInNewContext(helpers.createCodingPlanCredentialClearScript(), host);
    runInNewContext(
      helpers.createCodingPlanAuthInjectionScript({
        provider,
        credentials: {
          zaiAccessToken: "test-zai",
          bigmodelAccessToken: "test-bigmodel",
          zcodeJwtToken: "test-jwt",
        },
        theme: "zai-dark",
        locale: "zh-CN",
      }),
      host,
    );
    assert.equal(values.get(`oauth:${provider}:access_token`), `test-${provider}`);
    assert.equal(values.get("zcodejwttoken"), "test-jwt");
    assert.equal(
      values.has(`oauth:${provider === "zai" ? "bigmodel" : "zai"}:access_token`),
      false,
    );
    assert.equal(host.window.__zcodeLang__, "zh-CN");
    assert.equal(classes.get("dark"), true);
    assert.equal(events[0].type, "zcode-coding-plan-auth-ready");
    assert.deepEqual(JSON.parse(JSON.stringify(events[0].detail)), { provider, locale: "zh-CN" });
    assert.equal(values.has("zcode:coding-plan:report-context"), false);
    assert.equal("__zcodeReportContext__" in host.window, false);
  }
});
