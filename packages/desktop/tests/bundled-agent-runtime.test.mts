import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import {
  BUNDLED_AGENT_ENTRY_RELATIVE,
  applyBundledAgentRuntime,
  resolveBundledAgentRuntime,
} from "../src/host-standalone/bundledAgentRuntime.js";

const RESOURCES = "C:\Program Files\Mode";
const ENTRY = join(RESOURCES, BUNDLED_AGENT_ENTRY_RELATIVE);
const PROVIDER = join(RESOURCES, "config", "provider", "mode-builtin.json");

test("随包 agent：显式 MODE_AGENT_SERVER_COMMAND 优先，不覆盖", () => {
  const env = { MODE_AGENT_SERVER_COMMAND: "custom-node", MODE_HOST_RESOURCES_DIR: RESOURCES };
  assert.equal(resolveBundledAgentRuntime({ env, execPath: "node.exe", exists: () => true }), null);
});

test("随包 agent：无资源目录或入口文件不存在时不注入", () => {
  assert.equal(
    resolveBundledAgentRuntime({ env: {}, execPath: "node.exe", exists: () => true }),
    null,
  );
  assert.equal(
    resolveBundledAgentRuntime({
      env: { MODE_HOST_RESOURCES_DIR: RESOURCES },
      execPath: "node.exe",
      exists: () => false,
    }),
    null,
  );
});

test("随包 agent：命中时用当前 Node 执行 agent/glm/mode.cjs app-server", () => {
  const runtime = resolveBundledAgentRuntime({
    env: { MODE_HOST_RESOURCES_DIR: RESOURCES },
    execPath: "C:\Program Files\Mode\node\node.exe",
    exists: (path) => path === ENTRY,
  });
  assert.ok(runtime);
  assert.equal(runtime.command, "C:\Program Files\Mode\node\node.exe");
  assert.deepEqual(JSON.parse(runtime.argsJson), [ENTRY, "app-server", "--stdio"]);
});

test("随包 agent：apply 写回两条环境变量；未命中时不改环境", () => {
  const env: NodeJS.ProcessEnv = { MODE_HOST_RESOURCES_DIR: RESOURCES };
  assert.equal(
    applyBundledAgentRuntime(env, "node.exe", (path) => path === ENTRY),
    true,
  );
  assert.equal(env.MODE_AGENT_SERVER_COMMAND, "node.exe");
  assert.deepEqual(JSON.parse(env.MODE_AGENT_SERVER_ARGS_JSON ?? "[]"), [
    ENTRY,
    "app-server",
    "--stdio",
  ]);
  assert.equal(env.MODE_BUILTIN_PROVIDER_CONFIG_FILE, undefined);

  const untouched: NodeJS.ProcessEnv = { MODE_HOST_RESOURCES_DIR: RESOURCES };
  assert.equal(
    applyBundledAgentRuntime(untouched, "node.exe", () => false),
    false,
  );
  assert.equal(untouched.MODE_AGENT_SERVER_COMMAND, undefined);
});

test("随包 agent：存在随包 provider config 且未显式设置时一并注入", () => {
  const env: NodeJS.ProcessEnv = { MODE_HOST_RESOURCES_DIR: RESOURCES };
  assert.equal(
    applyBundledAgentRuntime(env, "node.exe", (path) => path === ENTRY || path === PROVIDER),
    true,
  );
  assert.equal(env.MODE_BUILTIN_PROVIDER_CONFIG_FILE, PROVIDER);

  const explicit: NodeJS.ProcessEnv = {
    MODE_HOST_RESOURCES_DIR: RESOURCES,
    MODE_BUILTIN_PROVIDER_CONFIG_FILE: "D:\custom\provider.json",
  };
  applyBundledAgentRuntime(explicit, "node.exe", (path) => path === ENTRY || path === PROVIDER);
  assert.equal(explicit.MODE_BUILTIN_PROVIDER_CONFIG_FILE, "D:\custom\provider.json");
});
