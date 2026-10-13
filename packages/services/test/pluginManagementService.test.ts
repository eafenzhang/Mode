import { describe, expect, it, vi } from "vitest";
import { createPluginManagementService } from "../src/plugins/pluginManagementService.js";
import type { IModeAgentService } from "../src/mode-agent/modeAgent.js";

describe("pluginManagementService suggested prompt operations", () => {
  it("forwards trusted resolution and cancellation to the selected Agent service", async () => {
    const progressEvent = vi.fn();
    const onDynamicPluginOperationProgress = vi.fn(() => progressEvent);
    const resolveSuggestedPluginReference = vi.fn(async () => ({
      stableId: "document-skills@mode-plugins-official",
      status: "missing" as const,
      marketplace: "mode-plugins-official",
      pluginName: "document-skills",
      sourceTrust: "official" as const,
      diagnostics: [],
    }));
    const cancelPluginOperation = vi.fn(async ({ operationId }: { operationId: string }) => ({
      operationId,
      cancelled: true,
    }));
    const service = createPluginManagementService({
      modeAgentService: {
        resolveSuggestedPluginReference,
        cancelPluginOperation,
        onDynamicPluginOperationProgress,
      } as unknown as IModeAgentService,
    });

    expect(service.onDynamicPluginOperationProgress("suggested-1")).toBe(progressEvent);

    await expect(
      service.resolveSuggestedPluginReference({
        workspacePath: "/remote/workspace",
        workspaceIdentity: "ssh://host/remote/workspace",
        remoteSessionId: "remote-session-1",
        stableId: "document-skills@mode-plugins-official",
        operationId: "suggested-1",
        clientMode: "web-remote-replayable",
        deliveryKind: "web-remote-replayable",
      }),
    ).resolves.toMatchObject({ status: "missing" });
    await expect(
      service.cancelPluginOperation({ operationId: "suggested-1" }),
    ).resolves.toEqual({ operationId: "suggested-1", cancelled: true });

    expect(resolveSuggestedPluginReference).toHaveBeenCalledWith({
      workspacePath: "/remote/workspace",
      workspaceIdentity: "ssh://host/remote/workspace",
      remoteSessionId: "remote-session-1",
      stableId: "document-skills@mode-plugins-official",
      operationId: "suggested-1",
      clientMode: "web-remote-replayable",
      deliveryKind: "web-remote-replayable",
    });
    expect(cancelPluginOperation).toHaveBeenCalledWith({ operationId: "suggested-1" });
    expect(onDynamicPluginOperationProgress).toHaveBeenCalledWith("suggested-1");
  });
});
