/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明环境矩阵，拆分会降低可核对性。 */
/**
 * 生成 `runtime-env` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/runtimeEnv.ts`
 *
 * 每个 sanitize 用例前重置模块级 CUA 凭据捕获；序列用例保留跨步骤状态。
 * `buildPassthrough` 的 JSON 字符串在 Rust 侧按解析后的对象比较（键序无语义）。
 *
 * ```bash
 * pnpm runtime-env:fixtures
 * cargo test -p runtime-env
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { isEffectiveDevelopmentNodeEnv } from "../packages/services/src/runtime-tools/nodeEnv.js";
import {
  buildAgentEndpointOriginEnv,
  buildAgentRuntimeEnv,
  buildAgentWorkspaceIdentityEnv,
} from "../packages/services/src/runtime-tools/agentProxyEnv.js";
import {
  buildModeToolEnvPassthroughEnv,
  getCapturedModeCuaBrokerCredentials,
  isCuaDevModeRequested,
  isModeCuaInternalFeatureEnabled,
  normalizeModeRuntimeEnv,
  readModeToolEnvPassthroughEnv,
  resetCapturedModeCuaBrokerCredentialsForTest,
  resolveModeRuntimeEnv,
  sanitizeModeRuntimeEnv,
  sanitizeModeRuntimeEnvInPlace,
  shouldCaptureModeToolEnvPassthroughKey,
  shouldSanitizeModeRuntimeEnvKey,
} from "../packages/shared/src/runtimeEnv.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "runtime-env", "fixtures", "runtime-env.json");

type Env = Record<string, string>;

const SANITIZE_CASES: Array<{ name: string; env: Env }> = [
  {
    name: "filters-proxy-and-telemetry",
    env: {
      PATH: "/usr/bin",
      NODE_ENV: "production",
      HTTP_PROXY: "http://proxy",
      https_proxy: "http://proxy",
      NPM_CONFIG_HTTP_PROXY: "http://proxy",
      NPM_CONFIG_USERCONFIG: "/home/.npmrc",
      OTEL_EXPORTER_OTLP_ENDPOINT: "http://otel",
      MODE_TELEMETRY_USER_ID: "user",
      MODE_RUNTIME_ENV: "development",
      MODE_AGENT_CA_CERT: "/ca.pem",
      YARN_CA: "/ca.pem",
      PNPM_CAFILE: "/ca.pem",
      YARN_REGISTRY: "https://registry",
    },
  },
  {
    name: "captures-broker-credentials",
    env: {
      MODE_CUA_PERMISSION_BROKER_SOCKET: " /tmp/broker.sock ",
      MODE_CUA_PLUGIN_AUTHORITY: " authority-1 ",
      MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER: " marker ",
      KEEP: "1",
    },
  },
  {
    name: "ignores-half-credentials",
    env: {
      MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker.sock",
    },
  },
  {
    name: "empty-broker-values",
    env: {
      MODE_CUA_PERMISSION_BROKER_SOCKET: "   ",
      MODE_CUA_PLUGIN_AUTHORITY: "",
    },
  },
];

const SEQUENCES: Array<{ name: string; steps: Env[] }> = [
  {
    name: "capture-then-clear-half",
    steps: [
      {
        MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/a.sock",
        MODE_CUA_PLUGIN_AUTHORITY: "auth-a",
      },
      { MODE_CUA_PLUGIN_AUTHORITY: "auth-b" },
    ],
  },
  {
    name: "unrelated-env-keeps-capture",
    steps: [
      {
        MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/a.sock",
        MODE_CUA_PLUGIN_AUTHORITY: "auth-a",
        MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER: "m1",
      },
      { PATH: "/bin" },
    ],
  },
];

const KEYS = [
  "PATH",
  "NODE_ENV",
  "HTTP_PROXY",
  "http_proxy",
  "NPM_CONFIG_HTTPS_PROXY",
  "npm_config_cafile",
  "YARN_PROXY",
  "pnpm_no_proxy",
  "YARN_REGISTRY",
  "OTEL_EXPORTER_OTLP_HEADERS",
  "MODE_TELEMETRY_DEVICE_MID",
  "MODE_TELEMETRY_USER_ID",
  "MODE_MODEL_TELEMETRY_ENABLED",
  "MODE_CUA_PERMISSION_BROKER_SOCKET",
  "MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER",
  "MODE_CUA_PLUGIN_AUTHORITY",
  "MODE_REMOTE_HTTP_PROXY",
  "MODE_REMOTE_NO_PROXY",
  "MODE_AGENT_CA_CERT",
];

const PASSTHROUGH_READS: Array<{ name: string; env: Env }> = [
  {
    name: "valid",
    env: {
      MODE_TOOL_ENV_PASSTHROUGH_JSON:
        '{"HTTP_PROXY":"http://proxy","NODE_ENV":"production","BAD-KEY":"x","OTEL_HEADERS":"x","MODE_AGENT_CA_CERT":"/ca.pem","NUM":1}',
    },
  },
  { name: "invalid-json", env: { MODE_TOOL_ENV_PASSTHROUGH_JSON: "not json" } },
  { name: "array", env: { MODE_TOOL_ENV_PASSTHROUGH_JSON: "[1,2]" } },
  { name: "empty", env: { MODE_TOOL_ENV_PASSTHROUGH_JSON: "" } },
  { name: "missing", env: { PATH: "/bin" } },
];

const PASSTHROUGH_BUILDS: Array<{ name: string; env: Env }> = [
  {
    name: "merges-existing-and-env",
    env: {
      MODE_TOOL_ENV_PASSTHROUGH_JSON: '{"HTTP_PROXY":"old","IGNORED":"x"}',
      HTTPS_PROXY: "new",
      NODE_ENV: "production",
      OTEL_EXPORTER_OTLP_ENDPOINT: "http://otel",
      PATH: "/bin",
    },
  },
  {
    name: "only-telemetry",
    env: { OTEL_EXPORTER_OTLP_ENDPOINT: "http://otel", PATH: "/bin" },
  },
  { name: "empty", env: {} },
];

function main(): void {
  const normalized = [
    "development",
    " PRODUCTION ",
    "test",
    "staging",
    null,
  ].map((value) => ({
    value,
    expected: normalizeModeRuntimeEnv(value ?? undefined) ?? null,
  }));

  const resolved = [
    { env: { MODE_RUNTIME_ENV: "development" } },
    { env: { MODE_RUNTIME_ENV: "TEST" } },
    { env: { MODE_RUNTIME_ENV: "staging" } },
    { env: {} },
    { env: {}, fallback: "test" },
  ].map(({ env, fallback }) => ({
    env,
    fallback: fallback ?? null,
    expected: resolveModeRuntimeEnv(env, fallback as never),
  }));

  const cua = [
    { MODE_CUA_DEV_MODE: "1" },
    { MODE_CUA_DEV_MODE: " TRUE " },
    { MODE_CUA_DEV_MODE: "on" },
    { MODE_CUA_DEV_MODE: "yes" },
    { MODE_CUA_PRODUCT_HELPER: "off" },
    { MODE_CUA_PRODUCT_HELPER: "0" },
    { MODE_CUA_PRODUCT_HELPER: "1" },
    {},
    { MODE_CUA_DEV_MODE: "1", MODE_CUA_PRODUCT_HELPER: "off" },
  ].map((env) => ({
    env,
    devMode: isCuaDevModeRequested(env),
    internalEnabled: isModeCuaInternalFeatureEnabled(env),
  }));

  const sanitize = SANITIZE_CASES.map((testCase) => {
    resetCapturedModeCuaBrokerCredentialsForTest();
    const sanitized = sanitizeModeRuntimeEnv(testCase.env);
    const captured = getCapturedModeCuaBrokerCredentials();
    const inPlace = { ...testCase.env };
    sanitizeModeRuntimeEnvInPlace(inPlace);
    return {
      name: testCase.name,
      env: testCase.env,
      expected: JSON.parse(JSON.stringify(sanitized)) as unknown,
      inPlace: JSON.parse(JSON.stringify(inPlace)) as unknown,
      captured: JSON.parse(JSON.stringify(captured)) as unknown,
    };
  });

  const sequences = SEQUENCES.map((sequence) => {
    resetCapturedModeCuaBrokerCredentialsForTest();
    return {
      name: sequence.name,
      steps: sequence.steps.map((env) => {
        const sanitized = sanitizeModeRuntimeEnv(env);
        return {
          env,
          expected: JSON.parse(JSON.stringify(sanitized)) as unknown,
          captured: JSON.parse(JSON.stringify(getCapturedModeCuaBrokerCredentials())) as unknown,
        };
      }),
    };
  });

  const keys = KEYS.map((key) => ({
    key,
    sanitize: shouldSanitizeModeRuntimeEnvKey(key),
    capture: shouldCaptureModeToolEnvPassthroughKey(key),
  }));

  const readPassthrough = PASSTHROUGH_READS.map((testCase) => ({
    name: testCase.name,
    env: testCase.env,
    expected: JSON.parse(
      JSON.stringify(readModeToolEnvPassthroughEnv(testCase.env)),
    ) as unknown,
  }));

  const buildPassthrough = PASSTHROUGH_BUILDS.map((testCase) => ({
    name: testCase.name,
    env: testCase.env,
    expected: JSON.parse(
      JSON.stringify(buildModeToolEnvPassthroughEnv(testCase.env)),
    ) as unknown,
  }));

  const fixture = {
    normalized,
    resolved,
    cua,
    sanitize,
    sequences,
    keys,
    readPassthrough,
    buildPassthrough,
    agentRuntime: [
      {},
      { httpProxy: "proxy.local:8080" },
      { httpProxy: " socks5://proxy:1080 " },
      { httpProxy: "HTTP://Proxy:3128" },
      { httpProxy: "   " },
      { httpProxy: "http://a", noProxy: " a, ,b , ", caCertPath: " /ca.pem " },
      { noProxy: ", ," },
      { caCertPath: "   " },
      { httpProxy: "://bad" },
    ].map((input) => ({
      input,
      expected: JSON.parse(JSON.stringify(buildAgentRuntimeEnv(input as never))) as unknown,
    })),
    agentEndpoint: [null, "", "  ", "https://api.example.com", " https://x "].map((input) => ({
      input,
      expected: JSON.parse(
        JSON.stringify(buildAgentEndpointOriginEnv(input ?? undefined)),
      ) as unknown,
    })),
    agentWorkspace: [null, "", " remote-ws ", "local"].map((input) => ({
      input,
      expected: JSON.parse(
        JSON.stringify(buildAgentWorkspaceIdentityEnv(input ?? undefined)),
      ) as unknown,
    })),
    nodeEnv: [
      {},
      { MODE_RUNTIME_ENV: "development" },
      { MODE_RUNTIME_ENV: " TEST " },
      { MODE_RUNTIME_ENV: "production" },
      { MODE_RUNTIME_ENV: "staging" },
      { NODE_ENV: "development" },
    ].map((env) => ({
      env,
      expected: isEffectiveDevelopmentNodeEnv(env),
    })),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

main();
