import assert from "node:assert/strict";
import test from "node:test";

import {
  runUpdateCheck,
  UPDATE_CHECK_TOAST_DEDUPE_KEY,
  updateCheckFeedback,
} from "../src/desktopUpdateFeedback.js";

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

test("检查更新：执行结果会转成 toast（修复「点了没反应」）", async () => {
  const shown: { message: string; options: unknown }[] = [];
  await runUpdateCheck({
    execute: async () => ({ status: "up-to-date", currentVersion: "0.0.60" }),
    formatMessage: format,
    showToast: (message, options) => shown.push({ message, options }),
  });
  assert.deepEqual(shown, [
    {
      message: 'update.toast.upToDate:{"version":"0.0.60"}',
      options: { variant: "info", dedupeKey: UPDATE_CHECK_TOAST_DEDUPE_KEY },
    },
  ]);
});

test("检查更新：执行抛错时提示失败，而不是静默", async () => {
  const shown: { message: string; options: unknown }[] = [];
  await runUpdateCheck({
    execute: async () => {
      throw new Error("ipc unavailable");
    },
    formatMessage: format,
    showToast: (message, options) => shown.push({ message, options }),
  });
  assert.deepEqual(shown, [
    {
      message: 'update.toast.error:{"error":"ipc unavailable"}',
      options: { variant: "warning", dedupeKey: UPDATE_CHECK_TOAST_DEDUPE_KEY },
    },
  ]);
});

test("检查更新：安装中进程会退出，不产生误导性 toast", async () => {
  const shown: unknown[] = [];
  await runUpdateCheck({
    execute: async () => ({ status: "installing", version: "0.0.60" }),
    formatMessage: format,
    showToast: (...args) => shown.push(args),
  });
  assert.deepEqual(shown, []);
});
