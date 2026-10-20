import assert from "node:assert/strict";
import test from "node:test";
import type { McpServerConfig, ModeMcpServer } from "@mode/shared";
import {
  buildDesktopEnrollmentUri,
  buildModeAgentLaunchPrompt,
  pickPreferredLanAddress,
  readModeAgentServers,
  renameModeAgentServerConfig,
} from "../src/remote-desktop/modeAgentPairing.js";

test("多网卡地址按 LAN → Tailscale → 链路本地排序", () => {
  assert.equal(
    pickPreferredLanAddress(["100.90.96.50:45880", "169.254.1.2:45880", "192.168.1.4:45880"]),
    "192.168.1.4:45880",
  );
  assert.equal(pickPreferredLanAddress(["100.90.96.50:45880", "10.0.0.5:45880"]), "10.0.0.5:45880");
  assert.equal(pickPreferredLanAddress(["169.254.1.2:45880"]), "169.254.1.2:45880");
  assert.equal(pickPreferredLanAddress([]), undefined);
});

test("配对串使用所选桌面地址与端口", () => {
  assert.equal(
    buildDesktopEnrollmentUri("192.168.1.4:45880", 45880, "A1B2C3"),
    "mode-agent://enroll?host=192.168.1.4&port=45880&code=A1B2C3",
  );
});

test("从 MCP 列表识别被控端并生成对话提示", () => {
  const server = {
    id: "zcodeagentmcp-global-mode-agent-abc123",
    name: "mode-agent-abc123",
    config: {
      type: "http",
      url: "http://192.168.1.20:47821/mcp",
      headers: { Authorization: "Bearer token" },
      modeAgent: {
        agentId: "abc123",
        name: "OFFICE-PC",
        host: "192.168.1.20",
        port: 47821,
        pairedAt: 1,
      },
    },
    enabled: true,
    source: "zcodeagentmcp",
    scope: "user",
  } satisfies ModeMcpServer;

  const records = readModeAgentServers([server]);
  assert.equal(records.length, 1);
  assert.equal(records[0]?.name, "OFFICE-PC");
  assert.equal(records[0]?.host, "192.168.1.20");
  assert.equal(records[0]?.port, 47821);

  const prompt = buildModeAgentLaunchPrompt(records[0]!);
  assert.match(prompt, /OFFICE-PC/);
  assert.match(prompt, /mcp__mode-agent-abc123__remote_\*/);
  assert.match(prompt, /remote_screen_capture/);
});

test("重命名只改展示名，保留连接配置", () => {
  const config = {
    type: "http",
    url: "http://192.168.1.20:47821/mcp",
    headers: { Authorization: "Bearer token" },
    timeoutMs: 180_000,
    modeAgent: {
      agentId: "abc123",
      name: "OFFICE-PC",
      host: "192.168.1.20",
      port: 47821,
      pairedAt: 5,
    },
  } satisfies McpServerConfig;
  const next = renameModeAgentServerConfig(
    config,
    { agentId: "abc123", host: "192.168.1.20", port: 47821 },
    " 新名字 ",
  );
  const metadata = (next as McpServerConfig & { modeAgent: { name: string; agentId: string } })
    .modeAgent;
  assert.equal(metadata.name, "新名字");
  assert.equal(metadata.agentId, "abc123");
  assert.equal(next.url, config.url);
  assert.deepEqual(next.headers, config.headers);
  assert.equal(next.timeoutMs, 180_000);
});
