/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明自适应码率、画质降档与会话统计矩阵。 */
/**
 * 生成 `remote-desktop-streaming` 的 TS 对拍数据。
 *
 * 真源：`packages/ui/src/remote-desktop/adaptiveBitrate.ts` 与
 * `packages/ui/src/remote-desktop/sessionStats.ts`。
 * 所有时钟与采样显式注入，fixture 可重复生成。
 *
 * ```bash
 * pnpm remote-desktop-streaming:fixtures
 * cargo test -p remote-desktop-streaming
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  ADAPTIVE_BITRATE_HIGH,
  ADAPTIVE_BITRATE_LOW,
  initialAdaptiveBitrateState,
  nextLowerQuality,
  stepAdaptiveBitrate,
  stepQualityDownshift,
  type AdaptiveBitrateSample,
  type AdaptiveBitrateState,
  type QualityDownshiftState,
} from "../packages/ui/src/remote-desktop/adaptiveBitrate.js";
import { createNativeSessionStatsCollector } from "../packages/ui/src/remote-desktop/sessionStats.js";
import type { RemoteDesktopQuality } from "../packages/shared/src/remote-desktop.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "remote-desktop-streaming",
  "fixtures",
  "adaptive.json",
);
const SESSION_STATS_OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "remote-desktop-streaming",
  "fixtures",
  "session-stats.json",
);

type Step = AdaptiveBitrateSample & { nowMs: number };

function initialCase(name: string, highBps?: number, lowBps?: number) {
  return {
    name,
    highBps: highBps ?? ADAPTIVE_BITRATE_HIGH,
    lowBps: lowBps ?? ADAPTIVE_BITRATE_LOW,
    expected:
      highBps === undefined && lowBps === undefined
        ? initialAdaptiveBitrateState()
        : initialAdaptiveBitrateState(highBps, lowBps),
  };
}

function adaptiveCase(name: string, initial: AdaptiveBitrateState, steps: Step[]) {
  let state = initial;
  const states: AdaptiveBitrateState[] = [];
  const nextBps: Array<number | null> = [];
  for (const step of steps) {
    const result = stepAdaptiveBitrate(state, step, step.nowMs);
    state = result.state;
    states.push(state);
    nextBps.push(result.nextBps);
  }
  return { name, initialState: initial, steps, states, nextBps };
}

function qualityCase(name: string, initial: QualityDownshiftState, steps: Step[]) {
  let state = initial;
  const states: QualityDownshiftState[] = [];
  const downshift: boolean[] = [];
  for (const step of steps) {
    const result = stepQualityDownshift(state, step, step.nowMs);
    state = result.state;
    states.push(state);
    downshift.push(result.downshift);
  }
  return { name, initialState: initial, steps, states, downshift };
}

const high = ADAPTIVE_BITRATE_HIGH;
const low = ADAPTIVE_BITRATE_LOW;

const lowState = (
  lastSwitchAt: number | null,
  healthyStreak: number,
): AdaptiveBitrateState => ({
  currentBps: low,
  highBps: high,
  lowBps: low,
  lastSwitchAt,
  healthyStreak,
});

const fixture = {
  initial: [
    initialCase("default"),
    initialCase("custom", 10_000_000, 2_000_000),
    initialCase("low-above-high", low, high),
  ],
  adaptive: [
    adaptiveCase("degrade-rtt-over-threshold", initialAdaptiveBitrateState(), [
      { rttMs: 61, droppedFrames: 0, nowMs: 1_000 },
    ]),
    adaptiveCase("rtt-boundary-60-is-not-congested", initialAdaptiveBitrateState(), [
      { rttMs: 60, droppedFrames: 0, nowMs: 1_000 },
      { rttMs: 30.5, droppedFrames: 0, nowMs: 2_000 },
      { rttMs: 30, droppedFrames: 0, nowMs: 3_000 },
    ]),
    adaptiveCase("degrade-dropped-frame", initialAdaptiveBitrateState(), [
      { rttMs: null, droppedFrames: 1, nowMs: 500 },
    ]),
    adaptiveCase(
      "low-already-does-not-reemit",
      lowState(1_000, 3),
      [{ rttMs: 500, droppedFrames: 0, nowMs: 9_000 }],
    ),
    adaptiveCase("recover-after-streak-and-dwell", lowState(1_000, 0), [
      { rttMs: 20, droppedFrames: 0, nowMs: 4_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 7_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 10_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 13_000 },
    ]),
    adaptiveCase("dwell-blocks-recovery-until-10s", lowState(1_000, 0), [
      { rttMs: 20, droppedFrames: 0, nowMs: 1_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 2_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 3_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 4_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 11_000 },
    ]),
    adaptiveCase("null-rtt-breaks-streak", lowState(null, 2), [
      { rttMs: null, droppedFrames: 0, nowMs: 1_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 2_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 3_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 4_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 5_000 },
    ]),
    adaptiveCase("recover-with-null-last-switch", lowState(null, 3), [
      { rttMs: 15, droppedFrames: 0, nowMs: 1_000 },
    ]),
    adaptiveCase("high-does-not-recover-again", initialAdaptiveBitrateState(), [
      { rttMs: 10, droppedFrames: 0, nowMs: 1_000 },
      { rttMs: 10, droppedFrames: 0, nowMs: 2_000 },
      { rttMs: 10, droppedFrames: 0, nowMs: 3_000 },
      { rttMs: 10, droppedFrames: 0, nowMs: 4_000 },
      { rttMs: 10, droppedFrames: 0, nowMs: 5_000 },
    ]),
  ],
  lowerQuality: (["sharp", "balanced", "smooth"] as RemoteDesktopQuality[]).map((quality) => ({
    quality,
    expected: nextLowerQuality(quality),
  })),
  qualityDownshift: [
    qualityCase(
      "thresholds-must-both-hold",
      { congestedStreak: 0, lastDownshiftAt: null },
      [
        { rttMs: 200, droppedFrames: 2, nowMs: 1_000 },
        { rttMs: 119, droppedFrames: 3, nowMs: 2_000 },
        { rttMs: null, droppedFrames: 5, nowMs: 3_000 },
        { rttMs: 120, droppedFrames: 3, nowMs: 4_000 },
      ],
    ),
    qualityCase("trigger-after-two-streak", { congestedStreak: 0, lastDownshiftAt: null }, [
      { rttMs: 150, droppedFrames: 4, nowMs: 1_000 },
      { rttMs: 150, droppedFrames: 4, nowMs: 4_000 },
    ]),
    qualityCase(
      "cooldown-blocks-then-allows",
      { congestedStreak: 0, lastDownshiftAt: null },
      [
        { rttMs: 150, droppedFrames: 4, nowMs: 1_000 },
        { rttMs: 150, droppedFrames: 4, nowMs: 2_000 },
        { rttMs: 150, droppedFrames: 4, nowMs: 10_000 },
        { rttMs: 150, droppedFrames: 4, nowMs: 20_000 },
        { rttMs: 150, droppedFrames: 4, nowMs: 32_000 },
      ],
    ),
    qualityCase("healthy-window-resets-streak", { congestedStreak: 0, lastDownshiftAt: null }, [
      { rttMs: 150, droppedFrames: 4, nowMs: 1_000 },
      { rttMs: 20, droppedFrames: 0, nowMs: 2_000 },
      { rttMs: 150, droppedFrames: 4, nowMs: 3_000 },
      { rttMs: 150, droppedFrames: 4, nowMs: 4_000 },
    ]),
    qualityCase("streak-kept-when-not-yet-triggered", { congestedStreak: 0, lastDownshiftAt: 1 }, [
      { rttMs: 120, droppedFrames: 3, nowMs: 2_000 },
      { rttMs: 120, droppedFrames: 3, nowMs: 3_000 },
    ]),
  ],
};

// ---- 会话状态条统计采集器（真源 sessionStats.ts）----

type StatsOp =
  | { op: "onDecoded" }
  | { op: "onPong"; rttMs: number }
  | { op: "onFrameAge"; ageMs: number }
  | { op: "onConfigured"; width: number; height: number }
  | { op: "onBytesReceived"; bytes: number }
  | { op: "onFrameDropped" }
  | { op: "onDecodeQueue"; size: number }
  | { op: "onFrozen" }
  | { op: "sample"; nowMs: number; expected: Record<string, unknown> };

function sessionStatsCase(name: string, ops: StatsOp[]) {
  let nowMs = 0;
  const collector = createNativeSessionStatsCollector({ now: () => nowMs });
  const recorded: StatsOp[] = [];
  for (const op of ops) {
    if (op.op === "sample") {
      nowMs = op.nowMs;
      const expected = collector.sample();
      recorded.push({ op: "sample", nowMs, expected });
    } else {
      switch (op.op) {
        case "onDecoded":
          collector.onDecoded();
          break;
        case "onPong":
          collector.onPong(op.rttMs);
          break;
        case "onFrameAge":
          collector.onFrameAge(op.ageMs);
          break;
        case "onConfigured":
          collector.onConfigured(op.width, op.height);
          break;
        case "onBytesReceived":
          collector.onBytesReceived(op.bytes);
          break;
        case "onFrameDropped":
          collector.onFrameDropped();
          break;
        case "onDecodeQueue":
          collector.onDecodeQueue(op.size);
          break;
        case "onFrozen":
          collector.onFrozen();
          break;
      }
      recorded.push(op);
    }
  }
  return { name, ops: recorded };
}

const sessionStatsFixture = {
  cases: [
    sessionStatsCase("fps-resets-between-samples", [
      ...Array.from({ length: 60 }, () => ({ op: "onDecoded" as const })),
      { op: "sample", nowMs: 0, expected: {} },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("fps-zero-before-any-decode", [
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("rtt-and-resolution-track-latest", [
      { op: "onPong", rttMs: 17 },
      { op: "onPong", rttMs: 19 },
      { op: "onConfigured", width: 1920, height: 1080 },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("empty-before-events", [
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("frame-age-median-odd-even-clears", [
      { op: "sample", nowMs: 0, expected: {} },
      { op: "onFrameAge", ageMs: 30 },
      { op: "onFrameAge", ageMs: 90 },
      { op: "onFrameAge", ageMs: 20 },
      { op: "sample", nowMs: 0, expected: {} },
      { op: "onFrameAge", ageMs: 10 },
      { op: "onFrameAge", ageMs: 21 },
      { op: "sample", nowMs: 0, expected: {} },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("resolution-follows-latest-config", [
      { op: "onConfigured", width: 1280, height: 720 },
      { op: "onConfigured", width: 2560, height: 1440 },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("bitrate-first-null-then-8000-then-0", [
      { op: "onBytesReceived", bytes: 999 },
      { op: "sample", nowMs: 1_000_000, expected: {} },
      { op: "onBytesReceived", bytes: 3_000_000 },
      { op: "sample", nowMs: 1_003_000, expected: {} },
      { op: "sample", nowMs: 1_006_000, expected: {} },
    ]),
    sessionStatsCase("decode-drops-reset-per-sample", [
      { op: "onFrameDropped" },
      { op: "onFrameDropped" },
      { op: "onFrameDropped" },
      { op: "sample", nowMs: 0, expected: {} },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("decode-queue-latest-or-null", [
      { op: "sample", nowMs: 0, expected: {} },
      { op: "onDecodeQueue", size: 1 },
      { op: "onDecodeQueue", size: 4 },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("freeze-count-accumulates", [
      { op: "onFrozen" },
      { op: "sample", nowMs: 0, expected: {} },
      { op: "onFrozen" },
      { op: "sample", nowMs: 0, expected: {} },
      { op: "sample", nowMs: 0, expected: {} },
    ]),
    sessionStatsCase("mixed-window-with-rounding-edges", [
      { op: "onPong", rttMs: 30.5 },
      { op: "onConfigured", width: 2560, height: 1440 },
      { op: "onFrameAge", ageMs: 15 },
      { op: "onFrameAge", ageMs: 16 },
      { op: "onFrameAge", ageMs: 90 },
      { op: "onBytesReceived", bytes: 3 },
      { op: "onDecodeQueue", size: 1.5 },
      { op: "onFrameDropped" },
      { op: "onFrozen" },
      { op: "sample", nowMs: 1_000, expected: {} },
      { op: "onDecoded" },
      { op: "onFrameAge", ageMs: 10 },
      { op: "onFrameAge", ageMs: 21 },
      { op: "onBytesReceived", bytes: 9 },
      { op: "sample", nowMs: 1_016, expected: {} },
      { op: "sample", nowMs: 1_100, expected: {} },
    ]),
  ],
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
mkdirSync(path.dirname(SESSION_STATS_OUTPUT), { recursive: true });
writeFileSync(
  SESSION_STATS_OUTPUT,
  `${JSON.stringify(sessionStatsFixture, null, 2)}\n`,
);
console.log(`wrote ${OUTPUT}`);
console.log(`wrote ${SESSION_STATS_OUTPUT}`);
