import { describe, expect, it } from "vitest";
import {
  getModeAgentAvailableModes,
  getModeAgentModeSelectOptions,
  normalizeAvailableModeMode,
} from "../src/mode-agent-model-state.js";
import { submissionModeSchema } from "../src/mode-protocol-v4/submission.js";
import { modeSessionModeSchema } from "../src/mode-protocol-legacy-types.js";
import { executionStateSchema, resolveExecutionState } from "../src/execution-state.js";

describe("guarded wire and mode catalog", () => {
  it("uses autonomous display copy without renaming the guarded value", () => {
    const copy = { name: "Autonomous mode", description: "Ask when there’s risk" };
    expect(getModeAgentAvailableModes().find((mode) => mode.id === "guarded")).toEqual({
      id: "guarded",
      ...copy,
    });
    expect(getModeAgentModeSelectOptions().find((option) => option.value === "guarded")).toEqual({
      value: "guarded",
      ...copy,
    });
  });
  it("preserves guarded across independent Plan transitions and serialization", () => {
    const current = resolveExecutionState({ mode: "guarded", planEnabled: true });
    expect(executionStateSchema.parse(current)).toEqual({ mode: "guarded", planEnabled: true });
    expect(resolveExecutionState({ planEnabled: false }, current)).toEqual({
      mode: "guarded",
      planEnabled: false,
    });
  });
  it("round trips the opt-in value without changing the default", () => {
    expect(submissionModeSchema.parse("guarded")).toBe("guarded");
    expect(modeSessionModeSchema.parse("guarded")).toBe("guarded");
    expect(normalizeAvailableModeMode("guarded")).toBe("guarded");
    const ids = getModeAgentAvailableModes().map((mode) => mode.id);
    expect(ids[0]).toBe("build");
    expect(ids).toContain("guarded");
    expect(ids).toEqual(["build", "guarded", "plan", "yolo"]);
    expect(getModeAgentModeSelectOptions().map((option) => option.value)).toEqual(ids);
    expect(submissionModeSchema.safeParse("unknown").success).toBe(false);
  });
  it("keeps historical edit valid without advertising it as a new choice", () => {
    expect(submissionModeSchema.parse("edit")).toBe("edit");
    expect(modeSessionModeSchema.parse("edit")).toBe("edit");
    expect(normalizeAvailableModeMode("edit")).toBe("edit");
    expect(getModeAgentAvailableModes().some((mode) => mode.id === "edit")).toBe(false);
  });
});
