import { describe, expect, it } from "vitest";
import {
  MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY,
  MODE_REMOTE_HTTP_PROXY_ENV_KEY,
  MODE_REMOTE_NO_PROXY_ENV_KEY,
  MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY,
  buildModeToolEnvPassthroughEnv,
  getCapturedModeCuaBrokerCredentials,
  isCuaDevModeRequested,
  isModeCuaInternalFeatureEnabled,
  readModeAgentTelemetryEnv,
  readModeToolEnvPassthroughEnv,
  resetCapturedModeCuaBrokerCredentialsForTest,
  sanitizeModeRuntimeEnv,
  getCapturedModeAgentTelemetryEnv,
  resetCapturedModeAgentTelemetryEnvForTest,
} from "../src/runtimeEnv.js";

describe("runtimeEnv", () => {
  it("enables internal CUA by default while honoring explicit false values", () => {
    expect(isModeCuaInternalFeatureEnabled({})).toBe(true);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: "0" })).toBe(false);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: "false" })).toBe(false);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: "1" })).toBe(true);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: " TRUE " })).toBe(true);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: "on" })).toBe(true);
  });

  it("MODE_CUA_DEV_MODE is the one-knob dev bundle and implies the internal feature flag", () => {
    // isCuaDevModeRequested: only explicit truthy values (case-insensitive, trimmed).
    expect(isCuaDevModeRequested({})).toBe(false);
    expect(isCuaDevModeRequested({ MODE_CUA_DEV_MODE: "0" })).toBe(false);
    expect(isCuaDevModeRequested({ MODE_CUA_DEV_MODE: "false" })).toBe(false);
    expect(isCuaDevModeRequested({ MODE_CUA_DEV_MODE: "1" })).toBe(true);
    expect(isCuaDevModeRequested({ MODE_CUA_DEV_MODE: " ON " })).toBe(true);
    expect(isCuaDevModeRequested({ MODE_CUA_DEV_MODE: "true" })).toBe(true);

    // DEV_MODE implies the internal feature flag without needing MODE_CUA_PRODUCT_HELPER,
    // collapsing the historical multi-var dev incantation to one knob.
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_DEV_MODE: "1" })).toBe(true);
    expect(isModeCuaInternalFeatureEnabled({ MODE_CUA_PRODUCT_HELPER: "1" })).toBe(true);
    expect(isModeCuaInternalFeatureEnabled({})).toBe(true);
  });

  it("sanitizes runtime env while sealing user network env for tools", () => {
    const sourceEnv = {
      HTTP_PROXY: "http://proxy.example:8080",
      NODE_ENV: "development",
      NODE_EXTRA_CA_CERTS: "/tmp/root-ca.pem",
      PATH: "/usr/bin:/bin",
      npm_config_proxy: "http://npm-proxy.example:8080",
    };

    const sanitized = sanitizeModeRuntimeEnv(sourceEnv);
    const passthroughEnv = buildModeToolEnvPassthroughEnv(sourceEnv);
    const captured = readModeToolEnvPassthroughEnv(passthroughEnv);

    expect(sanitized).toEqual({ PATH: "/usr/bin:/bin" });
    expect(passthroughEnv[MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY]).toBeDefined();
    expect(captured).toEqual({
      HTTP_PROXY: "http://proxy.example:8080",
      NODE_EXTRA_CA_CERTS: "/tmp/root-ca.pem",
      npm_config_proxy: "http://npm-proxy.example:8080",
    });
  });

  it("ignores invalid passthrough JSON and non-network keys", () => {
    const captured = readModeToolEnvPassthroughEnv({
      [MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY]: JSON.stringify({
        HTTP_PROXY: "http://proxy.example:8080",
        NODE_ENV: "production",
        PATH: "/tmp/bin",
      }),
    });

    expect(captured).toEqual({
      HTTP_PROXY: "http://proxy.example:8080",
    });
    expect(
      readModeToolEnvPassthroughEnv({
        [MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY]: "not-json",
      }),
    ).toEqual({});
  });

  it("strips the CUA broker socket/authority/refresh marker from child env and tool passthrough", () => {
    // Confused-deputy 防护：broker bearer 凭据只该定向注入目标 mode-cua server，绝不能随全局 env
    // 泄漏给其它 MCP / Bash / tool 子进程，也不能经 tool-env-passthrough 恢复。
    const sourceEnv = {
      PATH: "/usr/bin:/bin",
      MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/mode-cua-me/broker-abc.sock",
      MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER:
        "/tmp/mode-cua-me/broker-abc.sock.permission-refresh.json",
    };
    const sanitized = sanitizeModeRuntimeEnv(sourceEnv);
    expect(sanitized).toEqual({ PATH: "/usr/bin:/bin" });

    const captured = readModeToolEnvPassthroughEnv(buildModeToolEnvPassthroughEnv(sourceEnv));
    expect(captured).not.toHaveProperty("MODE_CUA_PERMISSION_BROKER_SOCKET");
    expect(captured).not.toHaveProperty("MODE_CUA_PERMISSION_BROKER_SOCKET");
    expect(captured).not.toHaveProperty("MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER");
  });

  it("strips desktop-to-remote network handoff values after stdio bootstrap", () => {
    const sourceEnv = {
      PATH: "/usr/bin:/bin",
      [MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY]: "1",
      [MODE_REMOTE_HTTP_PROXY_ENV_KEY]: "http://172.21.240.1:7890",
      [MODE_REMOTE_NO_PROXY_ENV_KEY]: "localhost,127.0.0.1",
    };

    expect(sanitizeModeRuntimeEnv(sourceEnv)).toEqual({ PATH: "/usr/bin:/bin" });
    expect(readModeToolEnvPassthroughEnv(buildModeToolEnvPassthroughEnv(sourceEnv))).toEqual({});
  });

  it("captures broker credentials as one atomic pair and clears stale state on partial rotation", () => {
    resetCapturedModeCuaBrokerCredentialsForTest();
    try {
      sanitizeModeRuntimeEnv({
        MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-a.sock",
        MODE_CUA_PLUGIN_AUTHORITY: "authority-a",
      });
      expect(getCapturedModeCuaBrokerCredentials()).toEqual({
        socket: "/tmp/broker-a.sock",
        pluginAuthority: "authority-a",
      });

      sanitizeModeRuntimeEnv({
        MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-b.sock",
      });
      expect(getCapturedModeCuaBrokerCredentials()).toEqual({
        socket: undefined,
        pluginAuthority: undefined,
      });

      sanitizeModeRuntimeEnv({
        MODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-b.sock",
        MODE_CUA_PLUGIN_AUTHORITY: "authority-b",
      });
      expect(getCapturedModeCuaBrokerCredentials()).toEqual({
        socket: "/tmp/broker-b.sock",
        pluginAuthority: "authority-b",
      });
    } finally {
      resetCapturedModeCuaBrokerCredentialsForTest();
    }
  });

  it("captures Agent OTLP config privately and never passes auth or identity through to tools", () => {
    resetCapturedModeAgentTelemetryEnvForTest();
    const sourceEnv = {
      OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: "https://arms.example.com/v1/traces",
      OTEL_EXPORTER_OTLP_TRACES_HEADERS: "Authorization=secret",
      OTEL_EXPORTER_OTLP_METRICS_ENDPOINT: "https://arms.example.com/v1/metrics",
      OTEL_EXPORTER_OTLP_METRICS_HEADERS: "Authorization=metric-secret",
      PATH: "/usr/bin:/bin",
      MODE_TELEMETRY_USER_SUBJECT_ID: "opaque-user",
    };
    expect(sanitizeModeRuntimeEnv(sourceEnv)).toEqual({ PATH: "/usr/bin:/bin" });
    expect(getCapturedModeAgentTelemetryEnv()).toMatchObject({
      OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: "https://arms.example.com/v1/traces",
      OTEL_EXPORTER_OTLP_TRACES_HEADERS: "Authorization=secret",
      OTEL_EXPORTER_OTLP_METRICS_ENDPOINT: "https://arms.example.com/v1/metrics",
      OTEL_EXPORTER_OTLP_METRICS_HEADERS: "Authorization=metric-secret",
      MODE_TELEMETRY_USER_SUBJECT_ID: "opaque-user",
    });
    const toolEnv = readModeToolEnvPassthroughEnv(buildModeToolEnvPassthroughEnv(sourceEnv));
    expect(toolEnv).not.toHaveProperty("OTEL_EXPORTER_OTLP_TRACES_HEADERS");
    expect(toolEnv).not.toHaveProperty("OTEL_EXPORTER_OTLP_METRICS_HEADERS");
    expect(toolEnv).not.toHaveProperty("MODE_TELEMETRY_USER_SUBJECT_ID");
  });

  it("extracts only the allowlisted Agent telemetry env", () => {
    expect(
      readModeAgentTelemetryEnv({
        OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: " https://arms.example.com/v1/traces ",
        OTEL_UNKNOWN_SECRET: "must-not-pass",
        PATH: "/bin",
        MODE_MODEL_TELEMETRY_ENABLED: "1",
        MODE_TELEMETRY_RUNTIME_DISTRIBUTION: " packaged ",
      }),
    ).toEqual({
      OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: "https://arms.example.com/v1/traces",
      MODE_MODEL_TELEMETRY_ENABLED: "1",
      MODE_TELEMETRY_RUNTIME_DISTRIBUTION: "packaged",
    });
  });
});
