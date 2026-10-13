import { describe, expect, it } from "vitest";
import type { ModeConfigOption } from "@mode/shared";
import { readCurrentModeMode } from "../src/hooks/chatActionHelpers.js";

function createModeOption(currentValue: string | boolean): ModeConfigOption {
  return {
    id: "mode",
    name: "Mode",
    category: "mode",
    type: "select",
    currentValue,
  };
}

describe("chatActionHelpers", () => {
  it("reads yolo from Mode Agent mode config options", () => {
    expect(readCurrentModeMode([createModeOption("yolo")])).toBe("yolo");
  });

  it("reads edit from Mode Agent mode config options", () => {
    expect(readCurrentModeMode([createModeOption("edit")])).toBe("edit");
  });

  it("keeps provider-native Mode Agent modes", () => {
    expect(readCurrentModeMode([createModeOption("bypassPermissions")])).toBe("bypassPermissions");
  });

  it("does not normalize retired provider-native agent mode ids", () => {
    const readMode = readCurrentModeMode as (
      options: readonly ModeConfigOption[],
      provider?: string,
    ) => unknown;
    expect(readCurrentModeMode([createModeOption("agent")])).toBeUndefined();
    expect(readMode([createModeOption("agent-full-access")], "gemini")).toBeUndefined();
  });

  it("ignores invalid mode values", () => {
    expect(readCurrentModeMode([createModeOption("unknown")])).toBeUndefined();
    expect(readCurrentModeMode([createModeOption(true)])).toBeUndefined();
  });
});
