# Spec：终端字体/主题 Profile（Rust `terminal-profile`）

状态：2026-11-02 第一版（纯组合层）。
真源：`packages/services/src/terminal/terminalProfile.ts` 的字体栈归一化与
`composeTerminalFontProfile`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `compose_terminal_font_profile` | `composeTerminalFontProfile` |
| `normalize_font_family` / `dedupe_font_family_stack` | 同名内部 helper |
| `parse_jsonc` / `read_terminal_config_path` | `parseJsonc` / `readNestedString` |
| `detect_windows_terminal_font_family_from_raw` | Windows Terminal 探测（注入 raw reader） |
| `detect_vs_code_terminal_font_family_from_raw` | VS Code/Insiders 探测 |
| `detect_kitty_font_family_from_raw` | kitty conf 探测 |
| `detect_iterm2_profile_from_raw` / `detect_macos_terminal_profile_from_raw` | macOS plist 原始探测 |
| `detect_alacritty_font_family_from_raw` | Alacritty TOML/YAML 探测 |
| `TerminalDetectedProfile` / `TerminalFontProfile` | 同名接口 |

系统字体/主题探测、JSONC/TOML/YAML 文件读取和 macOS `plutil` 调用仍由适配层负责；
Rust 只接收已探测到的 `TerminalDetectedProfile`。

## 2. 组合语义

默认字体栈（去重顺序）：
`ui-monospace, SFMono-Regular, SF Mono, Menlo, Monaco, Consolas, Cascadia Mono,
JetBrains Mono, MesloLGS NF, Hack Nerd Font, Noto Sans Mono CJK SC, monospace`。

1. 设置里的 `terminalFontFamily` trim 后非空 → `source=custom`，字体栈按逗号拆分、
   trim、去空，再追加缺失的 fallback；字号/主题继承 detected profile。
2. 否则 `terminalInheritSystemProfile !== false` 且 detected 存在 → `source=system`；
   字体族缺失时用 `ui-monospace`，字号/主题照常继承。
3. 否则 `source=fallback`，只输出完整 fallback 栈。

`terminalInheritSystemProfile` 缺省视为 true。detected profile 只有在字体族、字号或
主题任一存在时才算有效。

## 3. 系统探测（JSONC/conf 子集）

- 文件读取由调用方注入 `(path) -> raw | null`；Rust 不访问磁盘，也不读进程环境。
- `parse_jsonc`：先按原始 JSON 解析，失败后剥 `//`、`/* */` 与尾逗号再试；只接受对象。
- Windows Terminal：仅 `win32`；候选包目录 → `LocalState/settings.json`。优先 default
  profile 的 `font.face`，其次 profiles.defaults，最后 list 顺序。
- VS Code：`APPDATA`、`XDG_CONFIG_HOME`、home 下 Code / Code - Insiders 的
  `settings.json`，读 `terminal.integrated.fontFamily`。
- kitty：`XDG_CONFIG_HOME` 或 home `.config` 与 macOS 路径的 `kitty.conf`，取首个
  `font_family`，trim 后剥首尾引号。
- 路径拼接复刻 Windows `path.join` 的窄口径（统一 `\`、折叠重复分隔符、保留驱动器
  与前导 `\`）。

## 4. macOS plist 原始探测

- 输入是 `plutil -convert json` 产出的 JSON 值；Rust 只做纯解析，进程/文件路径由
  调用方注入。
- 字体：`MesloLGS-NF 14.5` → `MesloLGS NF` + `14.5`；字号只接受 parseFloat 后
  6..72；归档 `NS.data` 仅作为 macOS Terminal 的 fallback。
- 颜色：hex `#rgb/#rrggbb/#rrggbbaa` 与 `rgb(...)/rgba(...)` 原样保留；对象按
  `Red/Green/Blue Component` 等别名取值，支持 0..1、0..255、0..65535 三档；alpha
  <1 输出 `rgba(r, g, b, a)`（toFixed(3) 去尾零），否则输出 `#rrggbb`。
- iTerm2：`New Bookmarks` 中 `Default Bookmark === true` 的条目提前，其余保持原序；
  取第一个有字体/字号/主题的 profile。
- macOS Terminal：先 `Startup Window Settings`，再 `Default Window Settings`；
  字体依次 `FontName` → 归一 `Font` → archived `Font`；字号 `FontSize` → Font
  descriptor；主题用 `TextColor/BackgroundColor/CursorColor/SelectionColor` 与
  16 个 ANSI 颜色。

### Alacritty

- 候选顺序：`$XDG_CONFIG_HOME`（缺省 home `.config`）下的 `alacritty.toml`、
  home `.alacritty.toml`；随后 `alacritty.yml`、`alacritty.yaml`。
- TOML/YAML 必须解析成对象；读取 `font.normal.family`；解析失败或缺失时继续下一个
  候选。home 缺省使用系统 `homedir()`，Rust 侧不注入系统回退，夹具不覆盖空 env。

## 5. 已知差异

1. Rust 侧 `home` 无 OS 回退；空 HOME/USERPROFILE 的系统回退由调用方实现，夹具不覆盖。
2. `readMacOsArchivedFontName` 在现有实现里不是可达路径：`Font` 为字符串时
   normalize 后直接作为字体族返回（夹具覆盖该既有行为，未做“修复”）。
3. Rust 的 base64 解码只覆盖 STANDARD；plist 原始二进制仍由调用方 `plutil` 转换。

## 6. 验收

- `pnpm terminal-profile-macos:fixtures` →
  `crates/terminal-profile/fixtures/macos.json`
  - 10 条：iTerm2 default 提前/无 default/仅主题/非法字体/空 plist；Terminal startup
    /archived fallback/缺 settings 名/仅主题/空 plist。
- `pnpm terminal-profile-alacritty:fixtures` →
  `crates/terminal-profile/fixtures/alacritty.json`
  - 9 条：TOML XDG/home、YAML `.yml`/`.yaml`、TOML 缺字体或非法时回退、
    非对象/非法 YAML、无文件。
- `pnpm terminal-profile-detect:fixtures` →
  `crates/terminal-profile/fixtures/detect.json`
  - 9 条：Windows default/preview/非 win32，VS Code APPDATA/home/无文件，
    kitty XDG/home/无文件；JSONC 注释与尾逗号由 Windows Terminal 用例覆盖。
- `pnpm terminal-profile:fixtures` → `crates/terminal-profile/fixtures/compose.json`
  - 12 条：自定义字体（重复/空白/大小写不同）、无 detected、detected 仅字体/仅字号/
    仅主题/空对象/空主题、fallback 全量、fallback 已存在时不重复追加。
- `cargo test -p terminal-profile`（4 条 Rust 单测 + 4 条 TS 深比较回放）。
- `cargo clippy -p terminal-profile --all-targets -- -D warnings`。

## 7. TS 对拍入口

为可注入 detected profile，TS `terminalProfile.ts` 抽出并导出
`composeTerminalFontProfile({ settings, detectedProfile })`；
`resolveTerminalFontProfile` 仍负责系统探测并调用该函数，行为不变。
系统探测侧导出 `detect*FontFamilyFromRaw`，原 detector 仍读磁盘并包装同一实现。
