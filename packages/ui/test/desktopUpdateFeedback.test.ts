import assert from "node:assert/strict";
import test from "node:test";

import { updateCheckFeedback } from "../src/desktopUpdateFeedback.js";

const format = (id: string, values?: Record<string, string | number>) =>
  values ? `${id}:${JSON.stringify(values)}` : id;

test("检查更新反馈：已是最新版本", () => {
  assert.deepEqual(
    updateCheckFeedback({ status: "up-to-date", currentVersion: "0.0.58" }, format),
    {
      message: 'update.toast.upToDate:{"version":"0.0.58"}',
      variant: "info",
    },
  );
});

test("检查更新反馈：失败与 busy", () => {
  assert.deepEqual(updateCheckFeedback({ status: "error", error: "network down" }, format), {
    message: 'update.toast.error:{"error":"network down"}',
    variant: "warning",
  });
  assert.deepEqual(updateCheckFeedback({ status: "busy" }, format), {
    message: "desktopMenu.help.checkingForUpdates",
    variant: "info",
  });
});

test("检查更新反馈：非对象或未知状态不反馈", () => {
  assert.equal(updateCheckFeedback(undefined, format), null);
  assert.equal(updateCheckFeedback("ok", format), null);
  assert.equal(updateCheckFeedback({ status: "other" }, format), null);
});
