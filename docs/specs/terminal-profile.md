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

## 4. 已知差异

1. Alacritty（TOML/YAML）、macOS Terminal/iTerm2 plist、主题颜色解析尚未迁。
2. Windows Terminal 的 `fontSize/theme` 仍由 macOS 探测迁移后提供；当前探测只返回字体族。
3. 主题对象按键值原样透传，不做颜色归一化。

## 5. 验收

- `pnpm terminal-profile-detect:fixtures` →
  `crates/terminal-profile/fixtures/detect.json`
  - 9 条：Windows default/preview/非 win32，VS Code APPDATA/home/无文件，
    kitty XDG/home/无文件；JSONC 注释与尾逗号由 Windows Terminal 用例覆盖。
- `pnpm terminal-profile:fixtures` → `crates/terminal-profile/fixtures/compose.json`
  - 12 条：自定义字体（重复/空白/大小写不同）、无 detected、detected 仅字体/仅字号/
    仅主题/空对象/空主题、fallback 全量、fallback 已存在时不重复追加。
- `cargo test -p terminal-profile`（3 条 Rust 单测 + 2 条 TS 深比较回放）。
- `cargo clippy -p terminal-profile --all-targets -- -D warnings`。

## 6. TS 对拍入口

为可注入 detected profile，TS `terminalProfile.ts` 抽出并导出
`composeTerminalFontProfile({ settings, detectedProfile })`；
`resolveTerminalFontProfile` 仍负责系统探测并调用该函数，行为不变。
系统探测侧导出 `detect*FontFamilyFromRaw`，原 detector 仍读磁盘并包装同一实现。
