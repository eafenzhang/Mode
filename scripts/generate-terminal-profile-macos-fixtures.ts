/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 macOS 终端 plist 探测矩阵。 */
/**
 * 生成 `terminal-profile` 的 macOS Terminal / iTerm2 原始 plist 探测对拍数据。
 *
 * 真源：`packages/services/src/terminal/terminalProfileMacOs.ts` 的
 * `detectIterm2ProfileFromRaw` 与 `detectMacOsTerminalProfileFromRaw`。
 *
 * ```bash
 * pnpm terminal-profile-macos:fixtures
 * cargo test -p terminal-profile
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  detectIterm2ProfileFromRaw,
  detectMacOsTerminalProfileFromRaw,
} from "../packages/services/src/terminal/terminalProfileMacOs.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "terminal-profile", "fixtures", "macos.json");

const color = {
  "Red Component": 255,
  "Green Component": 0.5,
  "Blue Component": 32,
  "Alpha Component": 0.25,
};
const rgbaColor = { red: 10, green: 20, blue: 30, opacity: 0.5 };
const iterm2Profiles = [
  {
    Name: "not-default",
    "Normal Font": "JetBrainsMono-Regular 13",
    "Foreground Color": color,
    "Background Color": "#001122",
    "Cursor Color": color,
    "Ansi 0 Color": color,
    "ANSI 8 Color": color,
  },
  {
    Name: "default",
    "Default Bookmark": true,
    "Normal Font": "MesloLGS-NF 14.5",
    "Foreground Color": color,
    "Background Color": "#00112233",
    "Cursor Color": color,
    "Cursor Text Color": color,
    "Selection Color": rgbaColor,
    "Ansi 0 Color": color,
    "ANSI 8 Color": color,
    "Ansi 15 Color": color,
  },
];

const archivedFont = Buffer.from(
  "MacBook-Pro:MesloLGS NF Regular:1234567890",
  "latin1",
).toString("base64");

const terminalSettings = {
  "Startup Window Settings": "Startup",
  "Default Window Settings": "Default",
  Startup: {
    FontName: " SF Mono ",
    FontSize: "13",
    TextColor: color,
    BackgroundColor: "#aabbcc",
    CursorColor: rgbaColor,
    SelectionColor: color,
    ANSIBlackColor: color,
    ANSIBrightWhiteColor: "#ffffff",
  },
  Default: {
    Font: archivedFont,
    TextColor: color,
    BackgroundColor: "#00112233",
  },
};

const cases = [
  {
    name: "iterm2-default-bookmark-first",
    kind: "iterm2",
    plist: { "New Bookmarks": iterm2Profiles },
  },
  {
    name: "iterm2-no-default-order",
    kind: "iterm2",
    plist: { "New Bookmarks": iterm2Profiles.filter((profile) => !profile["Default Bookmark"]) },
  },
  {
    name: "iterm2-theme-only",
    kind: "iterm2",
    plist: { "New Bookmarks": [{ "Foreground Color": color }] },
  },
  {
    name: "iterm2-invalid-font-only",
    kind: "iterm2",
    plist: { "New Bookmarks": [{ "Normal Font": "   " }] },
  },
  {
    name: "iterm2-empty-bookmarks",
    kind: "iterm2",
    plist: {},
  },
  {
    name: "terminal-startup-settings",
    kind: "terminal",
    plist: terminalSettings,
  },
  {
    name: "terminal-archived-font-fallback",
    kind: "terminal",
    plist: {
      "Default Window Settings": "Default",
      Default: terminalSettings.Default,
    },
  },
  {
    name: "terminal-missing-settings-name",
    kind: "terminal",
    plist: { Default: terminalSettings.Default },
  },
  {
    name: "terminal-theme-only",
    kind: "terminal",
    plist: {
      "Startup Window Settings": "ThemeOnly",
      ThemeOnly: { TextColor: color, BackgroundColor: "#aabbcc" },
    },
  },
  {
    name: "terminal-empty",
    kind: "terminal",
    plist: {},
  },
];

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  kind: testCase.kind,
  plist: testCase.plist,
  expected:
    testCase.kind === "iterm2"
      ? detectIterm2ProfileFromRaw(testCase.plist)
      : detectMacOsTerminalProfileFromRaw(testCase.plist),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
