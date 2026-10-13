import { describe, expect, it } from "vitest";
import type { ModeSessionSettingsState } from "@mode/shared";
import {
  getModeAgentAvailableModes,
  normalizeAvailableModeMode,
  settingsToConfigOptions,
} from "../src/mode-agent/modeConfigOptions.js";

function settingsWithMode(
  mode: ModeSessionSettingsState["mode"]["current"],
): ModeSessionSettingsState {
  return {
    model: {
      current: { providerId: "glm", modelId: "glm-5" },
      available: [],
    },
    thoughtLevel: {
      enabled: false,
      available: [],
    },
    mode: { current: mode },
  };
}

describe("modeConfigOptions", () => {
  it("keeps persisted edit while the shared menu replaces it with guarded", () => {
    const options = settingsToConfigOptions(settingsWithMode("edit"));
    const modeOption = options.find((option) => option.category === "mode");

    expect(modeOption?.currentValue).toBe("edit");
    expect(modeOption?.options?.map((option) => option.value)).toEqual([
      "build",
      "guarded",
      "plan",
      "yolo",
    ]);
    expect(getModeAgentAvailableModes().map((mode) => mode.id)).toEqual([
      "build",
      "guarded",
      "plan",
      "yolo",
    ]);
    expect(normalizeAvailableModeMode("edit")).toBe("edit");
  });
});
