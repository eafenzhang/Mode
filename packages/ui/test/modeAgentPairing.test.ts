import assert from "node:assert/strict";
import test from "node:test";
import type { ModeMcpServer } from "@mode/shared";
import {
  buildModeAgentLaunchPrompt,
  buildModeAgentMcpConfig,
  modeAgentServerName,
  normalizeModeAgentCode,
  pairModeAgent,
  parseModeAgentPairingText,
  readModeAgentServers,
} from "../src/remote-desktop/modeAgentPairing.js";

test("配对码与配对信息解析", () => {
  assert.equal(normalizeModeAgentCode("ab cd-efgh"), "ABCDEFGH");
  assert.deepEqual(parseModeAgentPairingText("mode-agent://pair?host=192.168.1.20&port=47821&code=ABCD-EFGH"), {
    host: "192.168.1.20",
    port: 47821,
    code: "ABCDEFGH",
  });
  assert.deepEqual(parseModeAgentPairingText("10.0.0.5:47821 ABCD-EFGH"), {
    host: "10.0.0.5",
    port: 47821,
    code: "ABCDEFGH",
  });
  assert.equal(parseModeAgentPairingText("ABCD-EFGH").code, "ABCDEFGH");
});

test("配对请求与 MCP 配置", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const fakeFetch = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    return new Response(
      JSON.stringify({
        ok: true,
        result: {
          token: "agent-token",
          agentId: "abc123",
          name: "OFFICE-PC",
          version: "0.1.0",
          platform: "windows",
        },
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  const result = await pairModeAgent(
    { host: "192.168.1.20", port: 47821, code: "ABCD-EFGH", label: "desktop" },
    fakeFetch,
  );
  assert.equal(result.agentId, "abc123");
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url, "http://192.168.1.20:47821/v1/pair");
  assert.match(String(calls[0]?.init.body), /"code":"ABCDEFGH"/);

  const config = buildModeAgentMcpConfig(result, {
    host: "192.168.1.20",
    port: 47821,
    code: "ABCD-EFGH",
  });
  assert.equal(config.type, "http");
  assert.equal(config.url, "http://192.168.1.20:47821/mcp");
  assert.equal(config.timeoutMs, 180_000);
  assert.deepEqual(config.headers, { Authorization: "Bearer agent-token" });
  assert.equal((config.modeAgent as { name?: string }).name, "OFFICE-PC");
});

test("从 MCP 列表识别被控端并生成对话提示", () => {
  const server = {
    id: "zcodeagentmcp-global-mode-agent-abc123",
    name: modeAgentServerName("abc123"),
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

test("配对失败透出被控端错误", async () => {
  const fakeFetch = (async () =>
    new Response(JSON.stringify({ ok: false, error: { message: "invalid_pair_code" } }), {
      status: 401,
      headers: { "content-type": "application/json" },
    })) as typeof fetch;
  await assert.rejects(
    () => pairModeAgent({ host: "127.0.0.1", port: 47821, code: "ABCDEFGH" }, fakeFetch),
    /invalid_pair_code/,
  );
});
