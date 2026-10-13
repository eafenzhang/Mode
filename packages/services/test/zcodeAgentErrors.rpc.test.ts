import { ChannelClient, ChannelServer, createQueuePair, ProxyChannel } from "@mode/rpc";
import { describe, expect, it } from "vitest";
import { IModeAgentService } from "#src/mode-agent/modeAgent.js";
import {
  MODE_AGENT_MCP_STATUS_MODE_UNSUPPORTED_ERROR_CODE,
  ModeAgentMcpStatusModeUnsupportedError,
} from "#src/mode-agent/modeAgentErrors.js";

describe("Mode Agent RPC errors", () => {
  it("preserves the MCP status-only unsupported code across ChannelClient RPC", async () => {
    const [serverProtocol, clientProtocol] = createQueuePair();
    const server = new ChannelServer(serverProtocol, "test");
    const client = new ChannelClient(clientProtocol);
    server.registerChannel(
      IModeAgentService.channelName,
      ProxyChannel.fromService({
        async listMcpServerStatuses() {
          throw new ModeAgentMcpStatusModeUnsupportedError();
        },
      }),
    );
    const service = ProxyChannel.toService<IModeAgentService>(
      client.getChannel(IModeAgentService.channelName),
    );

    try {
      await expect(
        service.listMcpServerStatuses({
          mcpServers: [],
          mode: "status",
          workspacePath: "/tmp/workspace",
        }),
      ).rejects.toMatchObject({
        code: MODE_AGENT_MCP_STATUS_MODE_UNSUPPORTED_ERROR_CODE,
        name: "ModeAgentMcpStatusModeUnsupportedError",
      });
    } finally {
      client.dispose();
      server.dispose();
    }
  });
});
