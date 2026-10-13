import { describe, expect, it, vi } from "vitest";
import {
  modeProtocolMethods,
  modeProtocolNotifications,
  type ModePluginOperationProgressNotification,
} from "@mode/shared";

describe("modeAgentService Plugin operation progress", () => {
  it("projects a validated Agent progress notification by operationId", async () => {
    let notificationListener:
      | ((notification: { method: string; params?: unknown }) => void)
      | null = null;
    const request = vi.fn(async (method: string) => {
      if (method !== modeProtocolMethods.pluginsResolveSuggestedReference) {
        throw new Error(`Unexpected request method: ${method}`);
      }
      notificationListener?.({
        method: modeProtocolNotifications.pluginOperationProgress,
        params: { operationId: "suggested-progress", state: "refreshing" },
      });
      return {
        stableId: "document-skills@mode-plugins-official",
        status: "missing",
        marketplace: "mode-plugins-official",
        pluginName: "document-skills",
        sourceTrust: "official",
        diagnostics: [],
      };
    });

    vi.resetModules();
    vi.doMock("../src/mode-agent/modeAgentProcessManager.js", () => ({
      ModeAgentProcessManager: class {
        onRuntimeLifecycle() {
          return { dispose() {} };
        }
        onRuntimeRestarted() {
          return { dispose() {} };
        }
        async getClient() {
          return {
            request,
            transportKind: "stdio",
            onNotification: (
              listener: (notification: { method: string; params?: unknown }) => void,
            ) => {
              notificationListener = listener;
              return { dispose() {} };
            },
            onRequest: () => ({ dispose() {} }),
            onClose: () => ({ dispose() {} }),
          };
        }
        disposeAll() {}
        async disposeAllAndWait() {}
      },
    }));
    const { createModeAgentService } = await import("../src/mode-agent/modeAgentService.js");
    const service = createModeAgentService();
    const received: ModePluginOperationProgressNotification[] = [];
    const subscription = service.onDynamicPluginOperationProgress("suggested-progress")((event) =>
      received.push(event),
    );

    try {
      await service.resolveSuggestedPluginReference({
        workspacePath: "/workspace",
        stableId: "document-skills@mode-plugins-official",
        operationId: "suggested-progress",
        clientMode: "desktop-continuous",
        deliveryKind: "desktop-continuous",
      });

      expect(received).toEqual([{ operationId: "suggested-progress", state: "refreshing" }]);

      notificationListener?.({
        method: modeProtocolNotifications.pluginOperationProgress,
        params: {
          operationId: "suggested-progress",
          state: "refreshing",
          unexpected: true,
        },
      });
      expect(received).toHaveLength(1);
    } finally {
      subscription.dispose();
      await service.disposeAllAndWait();
    }
  });
});
