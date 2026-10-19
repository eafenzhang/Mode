import assert from "node:assert/strict";
import test from "node:test";

import {
  completeEffortCommand,
  completeModeCommand,
  completeModelCommand,
  completeSlashCommand,
  createCtrlCExitGuard,
  isModeSwitchKey,
  resolveCtrlCExitIntent,
  shouldClearPromptDraftOnCtrlC,
  shouldHandleInputHistoryNavigation,
  workflowExpansionActionFor,
} from "../src/app-keyboard-helpers.js";

test("completeSlashCommand：无候选/无选择不写草稿，有选择写入命令", () => {
  let draft = "before";
  assert.equal(
    completeSlashCommand([], undefined as never, (v) => (draft = v)),
    false,
  );
  assert.equal(draft, "before");

  assert.equal(
    completeSlashCommand(
      [{ name: "help" }] as never,
      { selectedIndex: 0 } as never,
      (v) => (draft = v),
    ),
    true,
  );
  assert.equal(draft, "/help ");
});

test("输入历史导航：空草稿或历史激活时可导航", () => {
  assert.equal(
    shouldHandleInputHistoryNavigation({ draftValue: "", inputHistoryActive: false }),
    true,
  );
  assert.equal(
    shouldHandleInputHistoryNavigation({ draftValue: "x", inputHistoryActive: false }),
    false,
  );
  assert.equal(
    shouldHandleInputHistoryNavigation({ draftValue: "x", inputHistoryActive: true }),
    true,
  );
});

test("workflow 展开：裸 +/- 且草稿为空、有卡片", () => {
  const key = (name: string, ctrl = false, meta = false) => ({ name, ctrl, meta }) as never;
  assert.equal(
    workflowExpansionActionFor({ key: key("+"), draftValue: "", hasCards: true }),
    "expand",
  );
  assert.equal(
    workflowExpansionActionFor({ key: key("-"), draftValue: "", hasCards: true }),
    "collapse",
  );
  assert.equal(
    workflowExpansionActionFor({ key: key("+"), draftValue: "x", hasCards: true }),
    undefined,
  );
  assert.equal(
    workflowExpansionActionFor({ key: key("+"), draftValue: "", hasCards: false }),
    undefined,
  );
  assert.equal(
    workflowExpansionActionFor({ key: key("-", true), draftValue: "", hasCards: true }),
    undefined,
  );
});

test("Ctrl-C：首按提示，窗口内再按确认退出，超时重新计时", () => {
  const guard = createCtrlCExitGuard();
  assert.equal(resolveCtrlCExitIntent(guard, 1000), "show_prompt");
  assert.equal(resolveCtrlCExitIntent(guard, 1500), "confirm_exit");
  assert.equal(resolveCtrlCExitIntent(guard, 5000), "show_prompt");
  assert.equal(shouldClearPromptDraftOnCtrlC(""), false);
  assert.equal(shouldClearPromptDraftOnCtrlC("x"), true);
});

test("模式切换键：Shift+Tab", () => {
  assert.equal(isModeSwitchKey({ name: "tab", shift: true } as never), true);
  assert.equal(isModeSwitchKey({ name: "tab", shift: false } as never), false);
});

test("命令补全：model/effort/mode 按选中项写回草稿，无选中或空列表不处理", () => {
  const models = [
    { ref: { providerId: "openai", modelId: "gpt-5" }, label: "GPT-5" },
    { ref: { providerId: "x", modelId: "y" }, label: "Y" },
  ] as never;
  const efforts = [{ id: "low" }, { id: "high" }] as never;
  const modes = [{ id: "plan" }, { id: "build" }] as never;

  let draft: string | undefined;
  assert.equal(
    completeModelCommand(models, { selectedIndex: 1 }, (value) => {
      draft = value;
    }),
    true,
  );
  assert.equal(draft, "/model x/y");

  assert.equal(
    completeModelCommand(models, { selectedIndex: 9 }, (value) => {
      draft = value;
    }),
    true,
  );
  assert.equal(draft, "/model x/y");

  assert.equal(
    completeModelCommand(models, undefined, () => {
      throw new Error("should not write");
    }),
    false,
  );
  assert.equal(
    completeModelCommand([] as never, { selectedIndex: 0 }, () => {
      throw new Error("should not write");
    }),
    false,
  );

  assert.equal(
    completeEffortCommand(efforts, { selectedIndex: 1 }, (value) => {
      draft = value;
    }),
    true,
  );
  assert.equal(draft, "/effort high");

  assert.equal(
    completeModeCommand(modes, { selectedIndex: 0 }, (value) => {
      draft = value;
    }),
    true,
  );
  assert.equal(draft, "/mode plan");
});
