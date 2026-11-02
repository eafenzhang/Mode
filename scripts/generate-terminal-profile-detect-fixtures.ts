/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明终端 profile 探测矩阵。 */
/**
 * 生成 `terminal-profile` 的系统探测对拍数据。
 *
 * 真源：`packages/services/src/terminal/terminalProfile.ts` 的
 * `detect*FontFamilyFromRaw`。
 *
 * ```bash
 * pnpm terminal-profile-detect:fixtures
 * cargo test -p terminal-profile
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  detectKittyFontFamilyFromRaw,
  detectVsCodeTerminalFontFamilyFromRaw,
  detectWindowsTerminalFontFamilyFromRaw,
} from "../packages/services/src/terminal/terminalProfile.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "terminal-profile", "fixtures", "detect.json");

const windowsSettings = `{
  // comments are stripped
  "defaultProfile": "{guid-a}",
  "profiles": {
    "defaults": { "font": { "face": "Default Font" } },
    "list": [
      { "guid": "{guid-b}", "font": { "face": "List Font" } },
      { "guid": "{guid-a}", "font": { "face": " Guid Font " }, },
    ],
  },
}`;

const vscodeSettings = `{
  "terminal.integrated.fontFamily": " VS Code Mono ",
}`;

const kittyConf = `# comment
font_family " Hack Nerd Font "
font_size 12
`;

const cases = [
  {
    name: "windows-default-profile-wins",
    kind: "windows",
    platform: "win32",
    env: { LOCALAPPDATA: "C:/Users/u/AppData/Local" },
    files: {
      "C:\\Users\\u\\AppData\\Local\\Packages\\Microsoft.WindowsTerminal_8wekyb3d8bbwe\\LocalState\\settings.json":
        windowsSettings,
    },
  },
  {
    name: "windows-preview-fallback",
    kind: "windows",
    platform: "win32",
    env: { LOCALAPPDATA: "C:/Users/u/AppData/Local" },
    files: {
      "C:\\Users\\u\\AppData\\Local\\Packages\\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\\LocalState\\settings.json":
        '{"profiles":{"list":[{"guid":"x","font":{"face":"Preview Font"}}]}}',
    },
  },
  {
    name: "windows-non-win32-empty",
    kind: "windows",
    platform: "linux",
    env: { LOCALAPPDATA: "C:/Users/u/AppData/Local" },
    files: {},
  },
  {
    name: "vscode-appdata",
    kind: "vscode",
    env: {
      HOME: "C:/Users/u",
      USERPROFILE: "C:/Users/u",
      APPDATA: "C:/Users/u/AppData/Roaming",
      XDG_CONFIG_HOME: "C:/xdg",
    },
    files: {
      "C:\\Users\\u\\AppData\\Roaming\\Code\\User\\settings.json": vscodeSettings,
    },
  },
  {
    name: "vscode-home-fallback",
    kind: "vscode",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\home\\u\\.config\\Code\\User\\settings.json": vscodeSettings,
    },
  },
  {
    name: "vscode-no-files",
    kind: "vscode",
    env: {},
    files: {},
  },
  {
    name: "kitty-xdg-quoted",
    kind: "kitty",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: { "\\xdg\\kitty\\kitty.conf": kittyConf },
  },
  {
    name: "kitty-home-fallback",
    kind: "kitty",
    env: { HOME: "/home/u" },
    files: { "\\home\\u\\.config\\kitty\\kitty.conf": "font_family   JetBrains Mono  \n" },
  },
  {
    name: "kitty-no-files",
    kind: "kitty",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {},
  },
];

function reader(files: Record<string, string>) {
  return (filePath: string) => files[filePath] ?? null;
}

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  kind: testCase.kind,
  platform: testCase.platform ?? "",
  env: testCase.env,
  files: testCase.files,
  expected:
    testCase.kind === "windows"
      ? detectWindowsTerminalFontFamilyFromRaw(
          testCase.platform as NodeJS.Platform,
          testCase.env as NodeJS.ProcessEnv,
          reader(testCase.files),
        )
      : testCase.kind === "vscode"
        ? detectVsCodeTerminalFontFamilyFromRaw(
            testCase.env as NodeJS.ProcessEnv,
            reader(testCase.files),
          )
        : detectKittyFontFamilyFromRaw(
            testCase.env as NodeJS.ProcessEnv,
            reader(testCase.files),
          ),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
