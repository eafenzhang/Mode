/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 Alacritty 探测矩阵。 */
/**
 * 生成 `terminal-profile` 的 Alacritty TOML/YAML 探测对拍数据。
 *
 * 真源：`packages/services/src/terminal/terminalProfile.ts` 的
 * `detectAlacrittyFontFamilyFromRaw`。
 *
 * ```bash
 * pnpm terminal-profile-alacritty:fixtures
 * cargo test -p terminal-profile
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { detectAlacrittyFontFamilyFromRaw } from "../packages/services/src/terminal/terminalProfile.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "terminal-profile",
  "fixtures",
  "alacritty.json",
);

const cases = [
  {
    name: "xdg-toml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.toml": "[font.normal]\nfamily = \" Tom TOML \"\n",
    },
  },
  {
    name: "home-toml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\home\\u\\.alacritty.toml": "[font.normal]\nfamily = 'Home TOML'\n",
    },
  },
  {
    name: "xdg-yml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.yml": "font:\n  normal:\n    family: \"YML Font\"\n",
    },
  },
  {
    name: "xdg-yaml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.yaml": "font:\n  normal:\n    family: YAML Font\n",
    },
  },
  {
    name: "toml-missing-font-falls-back-to-yaml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.toml": "[window]\nopacity = 0.5\n",
      "\\xdg\\alacritty\\alacritty.yml": "font:\n  normal:\n    family: YAML Font\n",
    },
  },
  {
    name: "invalid-toml-falls-back-to-yaml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.toml": "[font\nfamily = \"bad\"\n",
      "\\xdg\\alacritty\\alacritty.yml": "font:\n  normal:\n    family: YAML Font\n",
    },
  },
  {
    name: "yaml-non-object-ignored",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.yml": "- just\n- list\n",
    },
  },
  {
    name: "invalid-yaml",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {
      "\\xdg\\alacritty\\alacritty.yml": "font:\n  normal: [unclosed\n",
    },
  },
  {
    name: "no-files",
    env: { HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" },
    files: {},
  },
];

function reader(files: Record<string, string>) {
  return (filePath: string) => files[filePath] ?? null;
}

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  env: testCase.env,
  files: testCase.files,
  expected: detectAlacrittyFontFamilyFromRaw(
    testCase.env as NodeJS.ProcessEnv,
    reader(testCase.files),
  ),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
