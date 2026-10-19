import assert from "node:assert/strict";
import test from "node:test";

import {
  approvalDecisionLabel,
  approvalPermissionScopes,
  approvalRequestDescription,
  isOfficialCuaProjectApproval,
  previewPermissionInput,
} from "../src/app-approval.js";

test("审批决策文案：仅识别显式 allow 值，其余回退 Deny", () => {
  assert.equal(approvalDecisionLabel("allow_once"), "Allow once");
  assert.equal(approvalDecisionLabel("allow_project"), "Always allow in this project");
  assert.equal(approvalDecisionLabel("deny"), "Deny");
  assert.equal(approvalDecisionLabel("allow_always"), "Deny");
  assert.equal(approvalDecisionLabel(undefined), "Deny");
});

test("权限输入预览：字符串原样、其余转为紧凑 JSON 文本", () => {
  assert.equal(previewPermissionInput("hello"), "hello");
  assert.equal(previewPermissionInput(42), "42");
  assert.equal(previewPermissionInput(null), "null");
  assert.equal(previewPermissionInput({ a: 1, b: "two" }), '{"a":1,"b":"two"}');
});

test("权限输入预览：长文本不截断", () => {
  const long = "x".repeat(400);
  assert.equal(previewPermissionInput(long).length, 400);
});

const CUA_RULE = "mode:permission-capability:official_cua";

test("审批描述：优先 input.description，缺省回退 reason", () => {
  assert.equal(
    approvalRequestDescription({ input: { description: "run ls" }, reason: "fallback" } as never),
    "run ls",
  );
  assert.equal(approvalRequestDescription({ input: {}, reason: "fallback" } as never), "fallback");
});

test("官方 CUA 项目审批：allow + addRules 且规则命中官方能力", () => {
  assert.equal(
    isOfficialCuaProjectApproval({
      toolName: "x",
      input: {},
      suggestedPermissionUpdates: [
        { type: "addRules", behavior: "allow", rules: [{ toolName: CUA_RULE }] },
      ],
    } as never),
    true,
  );
  assert.equal(
    isOfficialCuaProjectApproval({ toolName: "Bash", input: { command: "ls" } } as never),
    false,
  );
  assert.equal(
    approvalDecisionLabel("allow_project", {
      toolName: "x",
      input: {},
      suggestedPermissionUpdates: [
        { type: "addRules", behavior: "allow", rules: [{ toolName: CUA_RULE }] },
      ],
    } as never),
    "Always allow Computer Use in this project",
  );
});

test("审批作用域：只取 Bash allow 规则，前缀规则用省略号，最多 5 条", () => {
  assert.deepEqual(
    approvalPermissionScopes({
      toolName: "Bash",
      input: {},
      suggestedPermissionUpdates: [
        {
          type: "addRules",
          behavior: "allow",
          rules: [
            { toolName: "Bash", ruleContent: "npm run test" },
            { toolName: "bash", ruleContent: "npm:*" },
            { toolName: "Read", ruleContent: "/a" },
            { toolName: "Bash", ruleContent: "" },
            { toolName: "Bash", ruleContent: "git status" },
          ],
        },
        {
          type: "addRules",
          behavior: "deny",
          rules: [{ toolName: "Bash", ruleContent: "rm -rf /" }],
        },
      ],
    } as never),
    [
      { kind: "exact", text: "npm run test" },
      { kind: "prefix", text: "npm …" },
      { kind: "exact", text: "git status" },
    ],
  );

  const many = approvalPermissionScopes({
    toolName: "Bash",
    input: {},
    suggestedPermissionUpdates: [
      {
        type: "addRules",
        behavior: "allow",
        rules: Array.from({ length: 7 }, (_, index) => ({
          toolName: "Bash",
          ruleContent: `cmd-${index}`,
        })),
      },
    ],
  } as never);
  assert.equal(many.length, 5);
  assert.equal(many[4]?.text, "cmd-4");
});

test("审批作用域：无 suggested updates 时从 input 取规则内容", () => {
  assert.deepEqual(
    approvalPermissionScopes({ toolName: "Bash", input: { command: "ls -la" } } as never),
    [{ kind: "exact", text: "ls -la" }],
  );
});
