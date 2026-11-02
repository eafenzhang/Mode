# Spec：终端启动解析（Rust `terminal-service-resolution`）

状态：2026-11-03 第一版（terminalService 纯解析子集）。
真源：`packages/services/src/terminal/terminalService.ts` 的
`resolveTerminal*With` / `shouldFallbackFromConptyDllWith`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `resolve_terminal_windows_pty_info_with` | `resolveTerminalWindowsPtyInfoWith` |
| `should_fallback_from_conpty_dll_with` | `shouldFallbackFromConptyDllWith` |
| `resolve_terminal_shell_with` | `resolveTerminalShellWith` |
| `resolve_terminal_cwd_with` | `resolveTerminalCwdWith` |
| `resolve_terminal_env_with` | `resolveTerminalEnvWith` |

PTY 进程、node-pty、macOS spawn-helper 修正与内存诊断不迁；可执行/目录检查、平台、
release、系统 home、环境与 PATH 分隔符由调用方注入。

## 2. 行为

- Windows PTY：仅 win32 返回 `{backend:"conpty", buildNumber}`；buildNumber 取
  `release` 第三段的前缀整数。
- ConPTY 回退：错误命中 `conpty.node module handle`、`conpty.node module file name`、
  `cannot find conpty.dll` 或 `error code:\s*126`（大小写不敏感）。
- shell：win32 顺序 `pwsh.exe`、`powershell.exe`、`ComSpec`、`cmd.exe`；其它平台
  `SHELL`、`/bin/zsh`、`/bin/bash`、`/bin/sh`；空候选跳过。
- cwd：`cwd` → `HOME` → 注入的系统 home → `/`；空/不可用候选跳过。
- env patch：
  - darwin 在传给终端的 env 里合并 PATH（用户 PATH 在前，再补 Homebrew/系统路径），
    分隔符由调用方注入，保持真实平台语义与夹具确定性；
  - `TERM=xterm-256color`；`COLORTERM` trim 后为空则 `truecolor`；
  - 仅当原 `TERM=dumb` 且现 `CI=1` 时删除 `CI`；
  - locale fallback 依次看 `LC_ALL/LC_CTYPE/LANG` 的 UTF-8，否则 darwin
    `en_US.UTF-8`、其它 `C.UTF-8`；`LANG/LC_CTYPE` 缺失或 C/POSIX 时补齐，
    `LC_ALL` 只有已存在且 C/POSIX 时覆盖。

## 3. 已知差异

1. Rust 不做 PATH 下候选文件的可执行检查；注入回调决定。
2. `pathDelimiter` 显式注入是为了让 darwin 行为在 Windows 夹具上确定；生产 TS 默认
   Node `path.delimiter`，Rust 调用方必须传真实平台分隔符。
3. 环境值只覆盖字符串；undefined 字段语义夹具不覆盖。

## 4. 验收

- `pnpm terminal-service-resolution:fixtures` →
  `crates/terminal-service-resolution/fixtures/resolution.json`
  - 16 条：Windows PTY、7 类 ConPTY 错误、Windows/Unix shell 成功与回退、
    cwd 首选/HOME/root/无可用、Linux locale+TERM、UTF-8 保留、darwin PATH+locale、
    darwin 无 PATH。
- `cargo test -p terminal-service-resolution`（3 条 Rust 单测 + 1 条 TS 回放）。
- `cargo clippy -p terminal-service-resolution --all-targets -- -D warnings`。
