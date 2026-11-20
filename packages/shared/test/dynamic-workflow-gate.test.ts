import assert from "node:assert/strict";
import test from "node:test";

import {
  createDynamicWorkflowClientConfig,
  resolveDynamicWorkflowGateWithTimeout,
} from "../src/dynamic-workflow-feature.js";

test("灰度读取超时 → fail-closed（不阻塞建会话）", async () => {
  const started = Date.now();
  const enabled = await resolveDynamicWorkflowGateWithTimeout(
    () => new Promise(() => {}),
    60,
  );
  assert.equal(enabled, false);
  assert.ok(Date.now() - started < 2_000, "超时必须及时返回");
});

test("启用态/缺省解析", async () => {
  assert.equal(
    await resolveDynamicWorkflowGateWithTimeout(async () =>
      createDynamicWorkflowClientConfig("alwaysOn", "remote"),
    ),
    true,
  );
  assert.equal(await resolveDynamicWorkflowGateWithTimeout(undefined), false);
});
