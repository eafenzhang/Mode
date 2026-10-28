import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { SessionDebugSnapshot } from "@mode/shared";

import { ConversationReplySpeed } from "../src/v4/ConversationReplySpeed.js";
import {
  collectConversationTurnWindows,
  formatReplySpeed,
  selectReplySpeedForTurn,
} from "../src/v4/conversationReplySpeedModel.js";
import type { ConversationTurnRenderUnit } from "../src/v4/conversationTurnRenderUnits.js";
import { ModeIntlProvider } from "../src/i18n/IntlProvider.js";

type DebugRound = SessionDebugSnapshot["rounds"][number];

function round(
  recordedAt: number,
  decode: number | null,
  prefill: number | null = null,
): DebugRound {
  return {
    eventKey: `event-${recordedAt}`,
    requestId: `request-${recordedAt}`,
    requestIndex: 1,
    recordedAt,
    usage: {},
    hitRate: null,
    generationDurationMs: 1000,
    tokensPerSecond: decode,
    prefillDurationMs: prefill === null ? null : 500,
    prefillTokensPerSecond: prefill,
  };
}

function unit(turnId: string, startedAt: number, endedAt?: number): ConversationTurnRenderUnit {
  return {
    turnId,
    startedAt,
    header: { startedAt, ...(endedAt === undefined ? {} : { endedAt }) },
  } as unknown as ConversationTurnRenderUnit;
}

test("按轮时间窗取最后一次带速度的 round", () => {
  const rounds = [round(1000, 50), round(1500, null, 800), round(2500, 99)];
  assert.deepEqual(selectReplySpeedForTurn(rounds, 900, 2000), {
    prefillTokensPerSecond: 800,
    decodeTokensPerSecond: null,
  });
});

test("窗口外与未完成的轮不返回速度", () => {
  const rounds = [round(1000, 50), round(3000, 60)];
  assert.equal(selectReplySpeedForTurn(rounds, 2000, 2500), null);
  assert.deepEqual(selectReplySpeedForTurn(rounds, 900, 2000), {
    prefillTokensPerSecond: null,
    decodeTokensPerSecond: 50,
  });
  assert.equal(selectReplySpeedForTurn(rounds, 900, undefined), null);
});

test("从 render unit 收集轮时间窗", () => {
  const windows = collectConversationTurnWindows([
    unit("a", 100, 200),
    unit("b", 300),
    unit("c", 400),
  ]);
  assert.deepEqual(windows, [
    { turnId: "a", startedAt: 100, endedAt: 200 },
    { turnId: "b", startedAt: 300 },
    { turnId: "c", startedAt: 400 },
  ]);
});

test("速度数字按 locale 格式化，未知值不渲染", () => {
  assert.equal(formatReplySpeed(1200.44, "en-US"), "1,200.4");
  const html = renderToStaticMarkup(
    createElement(
      ModeIntlProvider,
      { initialLocale: "en-US" },
      createElement(ConversationReplySpeed, {
        speed: { prefillTokensPerSecond: 1200.44, decodeTokensPerSecond: 88.6 },
      }),
    ),
  );
  assert.match(html, /↑1,200\.4 tok\/s/);
  assert.match(html, /↓88\.6 tok\/s/);
});

test("没有速度时整段省略", () => {
  const html = renderToStaticMarkup(
    createElement(
      ModeIntlProvider,
      { initialLocale: "en-US" },
      createElement(ConversationReplySpeed, { speed: null }),
    ),
  );
  assert.equal(html, "");
});
