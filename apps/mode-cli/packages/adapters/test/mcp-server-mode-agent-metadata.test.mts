// Mode Agent 设备元数据（docs/specs/remote-control-unified-pairing.md R2）：
// 桌面端把设备展示名/地址/配对时间写进同一个 MCP server 的 modeAgent 字段。
// CLI 配置入口是 strict 校验，若不认识该键会把整个 server 判为非法并跳过，
// 被控端就会从会话工具面消失（表现为 config.mcp_server.skipped、mcpServerCount: 0）。
import assert from "node:assert/strict";
import test from "node:test";

import { parseConfigFileToRuntimePatchWithDiagnostics } from "../dist/config/schema.js";

const SERVER_NAME = "mode-agent-e21b5d17214d8a4d";

function modeAgentServer(extra: Record<string, unknown> = {}) {
  return {
    type: "http",
    url: "http://192.168.1.4:47821/mcp",
    headers: { Authorization: "Bearer test-token" },
    timeoutMs: 180_000,
    modeAgent: {
      agentId: "e21b5d17214d8a4d0c00e7be556b2b6a",
      name: "EAFEN",
      host: "192.168.1.4",
      port: 47821,
      pairedAt: 1_792_504_598_155,
      version: "0.1.0",
      platform: "windows",
    },
    ...extra,
  };
}

test("带 modeAgent 元数据的 MCP server 保留在 runtime patch 中", () => {
  const parsed = parseConfigFileToRuntimePatchWithDiagnostics({
    mcp: { servers: { [SERVER_NAME]: modeAgentServer() } },
  });

  assert.deepEqual(parsed.diagnostics, []);
  const server = parsed.config.mcp?.servers?.[SERVER_NAME] as
    | { url?: string; modeAgent?: { name?: string } }
    | undefined;
  assert.ok(server, "server 必须保留，不能被 strict schema 跳过");
  assert.equal(server?.url, "http://192.168.1.4:47821/mcp");
  assert.equal(server?.modeAgent?.name, "EAFEN");
});

test("modeAgent 之外仍然拒绝未知键，避免无意放宽其它配置", () => {
  const parsed = parseConfigFileToRuntimePatchWithDiagnostics({
    mcp: {
      servers: {
        [SERVER_NAME]: modeAgentServer({ unknownField: true }),
      },
    },
  });

  assert.equal(parsed.diagnostics.length, 1);
  assert.equal(parsed.diagnostics[0]?.code, "config_mcp_server_invalid");
  assert.equal(parsed.config.mcp?.servers?.[SERVER_NAME], undefined);
});
