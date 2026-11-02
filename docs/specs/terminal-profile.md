# Spec：终端字体/主题 Profile（Rust `terminal-profile`）

状态：2026-11-02 第一版（纯组合层）。
真源：`packages/services/src/terminal/terminalProfile.ts` 的字体栈归一化与
`composeTerminalFontProfile`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `compose_terminal_font_profile` | `composeTerminalFontProfile` |
| `normalize_font_family` / `dedupe_font_family_stack` | 同名内部 helper |
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

## 3. 已知差异

1. 系统探测（Windows Terminal/VS Code/kitty/Alacritty/macOS Terminal/iTerm2）尚未迁；
    Rust 入口只做组合，探测结果由调用方注入。
2. 主题对象按键值原样透传，不做颜色归一化；颜色解析在 macOS plist 探测迁移时处理。

## 4. 验收

- `pnpm terminal-profile:fixtures` → `crates/terminal-profile/fixtures/compose.json`
  - 12 条：自定义字体（重复/空白/大小写不同）、无 detected、detected 仅字体/仅字号/
    仅主题/空对象/空主题、fallback 全量、fallback 已存在时不重复追加。
- `cargo test -p terminal-profile`（1 条 TS 深比较回放 + 2 条 Rust 单测）。
- `cargo clippy -p terminal-profile --all-targets -- -D warnings`。

## 5. TS 对拍入口

为可注入 detected profile，TS `terminalProfile.ts` 抽出并导出
`composeTerminalFontProfile({ settings, detectedProfile })`；
`resolveTerminalFontProfile` 仍负责系统探测并调用该函数，行为不变。
