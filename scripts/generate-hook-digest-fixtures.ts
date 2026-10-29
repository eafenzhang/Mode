import { writeFileSync } from "node:fs";
import { createWorkspaceHookDeclarationDigest } from "../packages/shared/src/workspace-hook-digest.js";

const cases = [
  { name: "command", hook: { type: "command", command: "npm test" }, timeoutMs: 60000 },
  {
    name: "command-shell-async",
    hook: { type: "command", command: "npm test", async: true, shell: "/bin/bash", timeout: 2 },
    timeoutMs: 60000,
  },
  { name: "process-args", hook: { type: "process", command: "node", args: ["a.js"] }, timeoutMs: 60000 },
];

const out = cases.map(({ name, hook, timeoutMs }) => ({
  name,
  hook,
  resolvedTimeoutMs: hook.timeout !== undefined ? hook.timeout * 1000 : timeoutMs,
  expected: createWorkspaceHookDeclarationDigest({
    sourceRelativePath: ".mode/hooks.json",
    sourceDiscoveryOrder: 0,
    event: "PreToolUse",
    matcher: null,
    matcherIndex: 0,
    hookIndex: 0,
    hook: hook as never,
    defaultTimeoutMs: timeoutMs,
    resolvedMaxOutputBytes: 1048576,
  }),
}));

writeFileSync("crates/workspace-hook-digest/tests/fixtures/entries.json", JSON.stringify({ cases: out }, null, 2) + "\n");
console.log(out.map((c) => `${c.name} ${c.expected}`).join("\n"));
