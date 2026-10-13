import { describe, expect, it } from "vitest";
import {
  shouldHideModeSessionForkAction,
  supportsModeSessionFork,
} from "../src/lib/modeProviders.js";

describe("supportsModeSessionFork", () => {
  it("所有 provider 都开放 session fork 入口", () => {
    expect(supportsModeSessionFork("glm")).toBe(true);
    expect(supportsModeSessionFork("claude")).toBe(true);
    expect(supportsModeSessionFork("codex")).toBe(true);
    expect(supportsModeSessionFork("gemini")).toBe(true);
    expect(supportsModeSessionFork("opencode")).toBe(true);
  });
});

describe("shouldHideModeSessionForkAction", () => {
  it("所有 provider 都不再隐藏 fork UI 入口", () => {
    expect(shouldHideModeSessionForkAction("glm")).toBe(false);
    expect(shouldHideModeSessionForkAction("claude")).toBe(false);
    expect(shouldHideModeSessionForkAction("codex")).toBe(false);
    expect(shouldHideModeSessionForkAction("gemini")).toBe(false);
    expect(shouldHideModeSessionForkAction("opencode")).toBe(false);
  });
});
