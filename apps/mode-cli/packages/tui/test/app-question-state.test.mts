import assert from "node:assert/strict";
import test from "node:test";

import { createQuestionPromptState } from "../src/app-question-state.js";

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
