/**
 * 会话面剪贴板同步（P1.1a：被控 → 主控）的行为回归。
 * 运行：pnpm --filter @mode/ui test
 * 语义真源是 shared 的回环抑制状态机；这里只钉「谁在什么情况下真的写本机剪贴板」。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { REMOTE_DESKTOP_CLIPBOARD_MAX_CHARS } from "@mode/shared/remote-desktop";
import { createRemoteDesktopClipboardApplier } from "@/remote-desktop/clipboardSync.js";

function createHarness(writeText?: (text: string) => Promise<void>) {
  const written: string[] = [];
  const errors: unknown[] = [];
  const applier = createRemoteDesktopClipboardApplier({
    writeText:
      writeText ??
      ((text) => {
        written.push(text);
        return Promise.resolve();
      }),
    onWriteError: (error) => errors.push(error),
  });
  return { applier, written, errors };
}

test("剪贴板下行：首次文本写本机，同值重发不再写", () => {
  const h = createHarness();
  h.applier.onControlMessage({ t: "clip", text: "来自对端" });
  assert.deepEqual(h.written, ["来自对端"]);
  h.applier.onControlMessage({ t: "clip", text: "来自对端" });
  assert.deepEqual(h.written, ["来自对端"], "重复值不重复写入（对端重发/回弹都要挡住）");
  h.applier.onControlMessage({ t: "clip", text: "变了" });
  assert.deepEqual(h.written, ["来自对端", "变了"]);
});

test("剪贴板下行：非 clip 事件与畸形载荷一律忽略", () => {
  const h = createHarness();
  h.applier.onControlMessage({ t: "freeze" });
  h.applier.onControlMessage({ t: "pong", ts: 1 });
  h.applier.onControlMessage({ t: "clip" });
  h.applier.onControlMessage({ t: "clip", text: 42 });
  h.applier.onControlMessage(null);
  h.applier.onControlMessage("clip");
  assert.deepEqual(h.written, []);
  assert.equal(h.errors.length, 0);
});

test("剪贴板下行：开关关闭时不写；重新打开后新值仍可写", () => {
  const h = createHarness();
  h.applier.setEnabled(false);
  h.applier.onControlMessage({ t: "clip", text: "关闭期" });
  assert.deepEqual(h.written, [], "关闭即不落地");
  h.applier.setEnabled(true);
  assert.equal(h.applier.isEnabled(), true);
  h.applier.onControlMessage({ t: "clip", text: "重开后" });
  assert.deepEqual(h.written, ["重开后"]);
});

test("剪贴板下行：超限文本不写（与 schema 同口径）", () => {
  const h = createHarness();
  h.applier.onControlMessage({
    t: "clip",
    text: "x".repeat(REMOTE_DESKTOP_CLIPBOARD_MAX_CHARS + 1),
  });
  assert.deepEqual(h.written, []);
});

test("剪贴板下行：写入失败上报错误但不抛出（不得影响画面与输入）", async () => {
  const failure = new Error("clipboard denied");
  const h = createHarness(() => Promise.reject(failure));
  h.applier.onControlMessage({ t: "clip", text: "写不进去" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(h.errors, [failure], "失败经 onWriteError 上报");
});

test("剪贴板上行（P1.1b）：本机变化上行一次；写进去的值与空文本都不上行", () => {
  const h = createHarness();
  assert.equal(h.applier.observeLocalText("本机复制的"), true);
  assert.equal(h.applier.observeLocalText("本机复制的"), false, "同值不重复上行");
  assert.equal(h.applier.observeLocalText(""), false, "空文本不参与同步");
  assert.equal(h.applier.observeLocalText(null), false, "读不到文本不算变化");
  // 下行写入本机剪贴板后，轮询会读到同一个值：必须被认作「刚应用过」，不得弹回对端。
  h.applier.onControlMessage({ t: "clip", text: "来自对端" });
  assert.equal(h.applier.observeLocalText("来自对端"), false, "刚写入的值不回弹");
  // 关闭后本机变化也不上行（开关是双向的）。
  h.applier.setEnabled(false);
  assert.equal(h.applier.observeLocalText("关闭期复制的"), false);
  h.applier.setEnabled(true);
  assert.equal(h.applier.observeLocalText("关闭期复制的"), true, "重开后该值相对 lastSynced 仍是新值");
});
