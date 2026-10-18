import assert from "node:assert/strict";
import test from "node:test";

import { spinnerFrame } from "../src/app-motion.js";

test("spinnerFrame：10 帧、80ms 步进、800ms 回到首帧", () => {
  assert.equal(spinnerFrame(0), "⠋");
  assert.equal(spinnerFrame(80), "⠙");
  assert.equal(spinnerFrame(400), "⠴");
  assert.equal(spinnerFrame(720), "⠏");
  assert.equal(spinnerFrame(800), "⠋");
  assert.equal(spinnerFrame(880), "⠙");
});
