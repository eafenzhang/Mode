/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明终端 profile 组合矩阵。 */
/**
 * 生成 `terminal-profile` 的 TS 对拍数据。
 *
 * 真源：`packages/services/src/terminal/terminalProfile.ts` 的
 * `composeTerminalFontProfile`。
 *
 * ```bash
 * pnpm terminal-profile:fixtures
 * cargo test -p terminal-profile
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { composeTerminalFontProfile } from "../packages/services/src/terminal/terminalProfile.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "terminal-profile", "fixtures", "compose.json");

const cases = [
  {
    name: "custom-with-detected",
    settings: { terminalFontFamily: " Menlo, ,JetBrains Mono,Menlo " },
    detectedProfile: {
      fontFamily: "Ignored",
      fontSize: 13.5,
      theme: { background: "#000000", foreground: "#ffffff" },
    },
  },
  {
    name: "custom-without-detected",
    settings: { terminalFontFamily: "Fira Code", terminalInheritSystemProfile: false },
    detectedProfile: null,
  },
  {
    name: "custom-whitespace-uses-detected",
    settings: { terminalFontFamily: "   " },
    detectedProfile: { fontFamily: "Menlo", fontSize: 11, theme: { cursor: "#fff" } },
  },
  {
    name: "detected-font-only",
    settings: {},
    detectedProfile: { fontFamily: " SF Mono " },
  },
  {
    name: "detected-size-only",
    settings: {},
    detectedProfile: { fontSize: 9 },
  },
  {
    name: "detected-theme-only",
    settings: {},
    detectedProfile: { theme: { background: "#111111", red: "#ff0000" } },
  },
  {
    name: "detected-empty-object",
    settings: {},
    detectedProfile: {},
  },
  {
    name: "empty-theme-object",
    settings: {},
    detectedProfile: { fontFamily: "Hack", theme: {} },
  },
  {
    name: "no-detected",
    settings: {},
    detectedProfile: null,
  },
  {
    name: "case-sensitive-dedupe",
    settings: { terminalFontFamily: "menlo, Menlo, monospace, Consolas" },
    detectedProfile: null,
  },
  {
    name: "detected-empty-font-with-size",
    settings: {},
    detectedProfile: { fontFamily: "", fontSize: 15 },
  },
  {
    name: "duplicates-fallbacks-already-present",
    settings: {
      terminalFontFamily:
        "ui-monospace, SFMono-Regular, SF Mono, Menlo, Monaco, Consolas, Cascadia Mono, JetBrains Mono, MesloLGS NF, Hack Nerd Font, Noto Sans Mono CJK SC, monospace",
    },
    detectedProfile: { fontFamily: "Ignored", fontSize: 10 },
  },
];

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  settings: testCase.settings,
  detectedProfile: testCase.detectedProfile,
  expected: composeTerminalFontProfile({
    settings: testCase.settings as never,
    detectedProfile: testCase.detectedProfile as never,
  }),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
