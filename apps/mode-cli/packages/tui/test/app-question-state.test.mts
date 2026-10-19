import assert from "node:assert/strict";
import test from "node:test";

import { createQuestionPromptState, handleQuestionKey } from "../src/app-question-state.js";

const INPUT = {
  questions: [
    {
      question: "Pick one?",
      header: "Pick",
      options: [
        { label: "A", description: "a" },
        { label: "B", description: "b" },
      ],
      multiSelect: false,
    },
  ],
} as never;

test("question prompt 初始状态：首题、首选项、非编辑/非 review、空答案", () => {
  const state = createQuestionPromptState(INPUT);
  assert.equal(state.currentQuestionIndex, 0);
  assert.equal(state.selectedOptionIndex, 0);
  assert.equal(state.editingOther, false);
  assert.equal(state.reviewing, false);
  assert.equal(state.otherBuffer, "");
  assert.deepEqual(state.answers, {});
  assert.deepEqual(state.annotations, {});
  assert.deepEqual(state.multiSelections, {});
  assert.deepEqual(state.otherText, {});
  assert.equal(state.input, INPUT);
});

interface QuestionRun {
  status?: string;
  queue: unknown;
  resolved?: Record<string, unknown>;
}

function runQuestionKey(key: Record<string, unknown>, questionState: unknown): QuestionRun {
  const approval = {
    questionState,
    resolve: (value: Record<string, unknown>) => {
      run.resolved = value;
    },
    reject: () => {},
    cleanup: () => {},
    request: {},
    selectedDecision: "allow_once",
  };
  const run: QuestionRun = { queue: undefined, resolved: undefined };
  handleQuestionKey(
    key as never,
    approval as never,
    ((value: unknown) => {
      run.queue =
        typeof value === "function"
          ? (value as (previous: unknown[]) => unknown)([approval])
          : value;
    }) as never,
    ((status: string) => {
      run.status = status;
    }) as never,
  );
  return run;
}

test("question prompt：Enter 选中当前项并进入 review", () => {
  const run = runQuestionKey({ name: "return", sequence: "\r" }, createQuestionPromptState(INPUT));
  const queue = run.queue as Array<{ questionState: ReturnType<typeof createQuestionPromptState> }>;
  assert.equal(queue.length, 1);
  assert.deepEqual(queue[0]?.questionState.answers, { "Pick one?": "A" });
  assert.equal(queue[0]?.questionState.reviewing, true);
  assert.equal(run.status, "Review answers, then press Enter to submit.");
});

test("question prompt：review 中再次 Enter 以 modify 决策提交答案", () => {
  const reviewing = {
    ...createQuestionPromptState(INPUT),
    answers: { "Pick one?": "A" },
    reviewing: true,
  };
  const run = runQuestionKey({ name: "return", sequence: "\r" }, reviewing);
  assert.deepEqual(run.queue, []);
  assert.equal(run.status, "Answered 1 clarification questions.");
  assert.equal(run.resolved?.decision, "modify");
  assert.equal(run.resolved?.reason, "Answered in TUI");
  const modified = run.resolved?.modifiedInput as {
    answers: Record<string, string>;
    questions: unknown[];
  };
  assert.deepEqual(modified.answers, { "Pick one?": "A" });
  assert.equal(modified.questions.length, 1);
});

test("question prompt：Escape 以 deny 决策放弃", () => {
  const run = runQuestionKey(
    { name: "escape", sequence: "\x1b" },
    createQuestionPromptState(INPUT),
  );
  assert.deepEqual(run.queue, []);
  assert.equal(run.status, "Clarification declined.");
  assert.equal(run.resolved?.decision, "deny");
  assert.equal(run.resolved?.reason, "Denied in TUI");
});

test("question prompt：缺少 questionState 时不处理且不改队列", () => {
  const run = runQuestionKey({ name: "return", sequence: "\r" }, undefined);
  assert.equal(run.queue, undefined);
  assert.equal(run.status, undefined);
  assert.equal(run.resolved, undefined);
});
