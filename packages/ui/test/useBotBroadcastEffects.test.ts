import { describe, expect, it, vi } from "vitest";
import type { ModeConfigOption } from "@mode/shared";
import {
  shouldRefreshBotTaskList,
  shouldMirrorBotTaskStreamToStore,
  syncBotTaskConfigOptionsToStore,
} from "../src/root/useBotBroadcastEffects.js";

describe("useBotBroadcastEffects", () => {
  it("refreshes created events even when task meta exists", () => {
    expect(shouldRefreshBotTaskList("created", true)).toBe(true);
  });

  it("keeps incremental-only handling for non-created events with task meta", () => {
    expect(shouldRefreshBotTaskList("updated", true)).toBe(false);
    expect(shouldRefreshBotTaskList("completed", true)).toBe(false);
    expect(shouldRefreshBotTaskList("error", true)).toBe(false);
  });

  it("mirrors active remote bot stream because ChatView may not subscribe to bot runtime", () => {
    expect(
      shouldMirrorBotTaskStreamToStore({
        activeTaskId: "task-1",
        taskId: "task-1",
        workspaceIdentity: "remote:docker:demo:/workspace/demo",
      }),
    ).toBe(true);
  });

  it("keeps skipping active local bot stream to avoid duplicate normal Mode Agent subscription writes", () => {
    expect(
      shouldMirrorBotTaskStreamToStore({
        activeTaskId: "task-1",
        taskId: "task-1",
      }),
    ).toBe(false);
  });

  it("syncs bot config options through the task config bucket", () => {
    const configOptions: ModeConfigOption[] = [
      {
        id: "runtime_mode",
        category: "mode",
        type: "select",
        currentValue: "plan",
        options: [
          { value: "default", name: "Default" },
          { value: "plan", name: "Plan" },
        ],
      },
    ];
    const modeSessionStore = {
      setTaskConfigOptions: vi.fn(),
    };

    syncBotTaskConfigOptionsToStore({
      modeSessionStore,
      workspacePath: "/repo",
      workspaceIdentity: "ssh://host/repo",
      taskId: "task-1",
      configOptions,
    });

    expect(modeSessionStore.setTaskConfigOptions).toHaveBeenCalledWith(
      "/repo",
      "task-1",
      configOptions,
      "ssh://host/repo",
    );
  });
});
