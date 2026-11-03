/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明自适应码率与画质降档矩阵。 */
/**
 * 生成 `remote-desktop-streaming` 的 TS 对拍数据。
 *
 * 真源：`packages/ui/src/remote-desktop/adaptiveBitrate.ts`。
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
import type { RemoteDesktopQuality } from "../packages/shared/src/remote-desktop.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "remote-desktop-streaming",
  "fixtures",
  "adaptive.json",
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

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
console.log(`wrote ${OUTPUT}`);
