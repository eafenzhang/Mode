import assert from "node:assert/strict";
import test from "node:test";

import { approvalDecisions } from "../src/app-model.js";

test("审批决策：与写入审批 spec 的三态一致且无独立全局允许", () => {
  // docs/plans/2026-10-17-agent-write-approval.md：options 只有三条，
  // allow_project 以 kind: allow_always 表达，不新增全局一直允许。
  assert.deepEqual(approvalDecisions, ["allow_once", "allow_project", "deny"]);
});
