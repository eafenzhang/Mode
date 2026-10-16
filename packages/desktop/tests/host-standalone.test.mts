/**
 * M3 Task 1 的**跨语言契约用例**（进 CI：`packages/desktop/tests/*.test.mts` 由发布流程的 verify 跑）。
 *
 * 这里守的是「两处各写一份、只有真跑才暴露」的三类漂移——它们都在真 webview 之外可判定：
 *
 * 1. **频道名**：注入桥（`crates/sidecar-host/src/bridge.ts`）与验收台面板
 *    （`apps/desktop-tauri/frontend/src/hostChannel.ts`）里的字面量必须等于
 *    `InternalChannels` 的真实取值（渲染层按这些常量收消息，写错一字即静默收不到）；
 * 2. **命令名**：桥脚本注入的命令名（`crates/sidecar-host/src/bridge.rs` 的常量）必须等于壳侧
 *    `#[tauri::command(rename = …)]` 的字面量（`apps/desktop-tauri/src-tauri/src/host.rs`）——
 *    Tauri v2 的命令名不会自动 camelCase，少一个 rename 就是运行时 `Command X not found`；
 * 3. **失败词汇表**：壳发布的 `phase` / `errorCode` 字面量（`crates/sidecar-host/src/startup.rs`）
 *    必须落在 `databaseStartupPhaseSchema` / `databaseStartupErrorCodeSchema` 的取值集合里——
 *    状态是 `.strict()` schema，多一个值/写错一个值会让**整条状态消息**被渲染层丢弃，
 *    症状是「闸门不放行、也没有失败原因」，最难查。
 *
 * 三条都是**读源码文本**的静态守卫（与 M1 `crates/platform/tests/commands.rs` 的扫源码守卫同一种做法）：
 * 单元测试跑不到另一门语言的字面量，而漂移一旦发生就是静默失效，所以宁可用文本断言换一声响。
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

import {
  databaseStartupErrorCodeSchema,
  databaseStartupPhaseSchema,
} from "@mode/shared";
import { InternalChannels } from "@mode/shared";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");

const read = (relativePath: string): string => readFileSync(resolve(repoRoot, relativePath), "utf8");

const BRIDGE_SCRIPT = "crates/sidecar-host/src/bridge.ts";
const BRIDGE_RUST = "crates/sidecar-host/src/bridge.rs";
const SHELL_HOST = "apps/desktop-tauri/src-tauri/src/host.rs";
const STARTUP_RUST = "crates/sidecar-host/src/startup.rs";
const BENCH_PANEL = "apps/desktop-tauri/frontend/src/hostChannel.ts";

test("注入桥与验收台面板用的频道名等于 InternalChannels 的真实取值", () => {
  const expectations: Array<[string, string]> = [
    [BRIDGE_SCRIPT, InternalChannels.ServicePort],
    [BRIDGE_SCRIPT, InternalChannels.DatabaseStartupState],
    [BENCH_PANEL, InternalChannels.ServicePort],
    [BENCH_PANEL, InternalChannels.DatabaseStartupState],
  ];
  for (const [file, channel] of expectations) {
    assert.ok(
      read(file).includes(`"${channel}"`),
      `${file} 里应出现字面量 "${channel}"（改名即与渲染层的收消息判定对不上）`,
    );
  }
  // 反向：桥脚本里的 mode: 名字只能是被允许的那几个（写错一个不会被正向断言抓到）。
  // 第三个是**桥自己的诊断事件**（`mode:host-bridge-error`），不是渲染层的频道——
  // 它只被验收台面板与集成测试页监听，与 InternalChannels 无关，故在这里显式列出。
  const referenced = [...read(BRIDGE_SCRIPT).matchAll(/"(mode:[a-z-]+)"/g)].map((match) => match[1]);
  const allowed = new Set<string>([
    InternalChannels.DatabaseStartupState,
    InternalChannels.ServicePort,
    "mode:host-bridge-error",
  ]);
  for (const name of new Set(referenced)) {
    assert.ok(allowed.has(name), `注入桥出现未登记的 mode: 名字 ${name}`);
  }
  for (const required of [InternalChannels.DatabaseStartupState, InternalChannels.ServicePort]) {
    assert.ok(referenced.includes(required), `注入桥必须引用 ${required}`);
  }
});

test("壳的命令 wire 名与注入桥里的命令名逐字一致", () => {
  const bridge = read(BRIDGE_RUST);
  const names = [...bridge.matchAll(/pub const CMD_\w+: &str = "([a-z_]+)"/g)].map((match) => match[1]);
  assert.equal(names.length, 3, `bridge.rs 里应有三个命令名常量，实际：${names.join(", ")}`);

  const shell = read(SHELL_HOST);
  const declared = [...shell.matchAll(/#\[tauri::command\(rename = "([a-z_]+)"\)\]/g)].map(
    (match) => match[1],
  );
  assert.deepEqual(
    [...declared].sort(),
    [...names].sort(),
    `host.rs 的 rename 字面量必须等于 bridge.rs 的命令名（Tauri 不会自动转换命令名）`,
  );
});

test("壳发布的 phase / errorCode 字面量都落在真 schema 的取值集合里", () => {
  const source = read(STARTUP_RUST);
  // `=> "字面量"` 只出现在 StartupPhase/StartupErrorCode 的 as_str 实现里。
  const literals = [...source.matchAll(/=> "([a-z_]+)"/g)].map((match) => match[1]);
  assert.ok(literals.length > 0, "startup.rs 里应能扫到 as_str 的字符串字面量");

  const phases = new Set<string>(databaseStartupPhaseSchema.options);
  const errorCodes = new Set<string>(databaseStartupErrorCodeSchema.options);
  for (const literal of literals) {
    assert.ok(
      phases.has(literal) || errorCodes.has(literal),
      `${literal} 不在 databaseStartupPhaseSchema / databaseStartupErrorCodeSchema 里——` +
        `状态是 .strict() schema，多一个取值会让整条状态消息被渲染层丢弃（症状：闸门不放行）`,
    );
  }
  // 正向覆盖：Task 1 用到的三类阶段与四个错误码都得有实现，别把词汇表删空还绿。
  for (const required of ["starting", "ready", "failed"]) {
    assert.ok(literals.includes(required), `StartupPhase 缺 ${required}`);
  }
  for (const required of [
    "open_failed",
    "transport_closed",
    "startup_status_timeout",
    "unsupported_runtime",
  ]) {
    assert.ok(literals.includes(required), `StartupErrorCode 缺 ${required}`);
  }
});
