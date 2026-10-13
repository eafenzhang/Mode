import { describe, expect, it } from "vitest";
import { MODE_AGENT_RUNTIME } from "@mode/shared";

describe("Mode Agent runtime", () => {
  it("starts the private Mode Protocol server instead of the retired Mode Agent command", () => {
    const retiredProtocolName = `${"a"}${"cp"}`;

    expect(MODE_AGENT_RUNTIME.spawnArgs).toEqual(["app-server", "--stdio"]);
    expect(MODE_AGENT_RUNTIME.spawnArgs).not.toContain(retiredProtocolName);
  });

  it("uses the Mode Agent binary name", () => {
    expect(MODE_AGENT_RUNTIME.resolveEntrySegments("darwin")).toEqual(["mode-agent"]);
    expect(MODE_AGENT_RUNTIME.resolveEntrySegments("win32")).toEqual(["mode-agent.exe"]);
  });
});
