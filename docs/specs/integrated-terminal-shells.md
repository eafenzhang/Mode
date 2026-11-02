# Spec：集成终端 shell 选项（Rust `integrated-terminal-shells`）

状态：2026-11-02 第一版。
真源：`packages/services/src/system/integratedTerminalShells.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `list_integrated_terminal_shell_options` | `listIntegratedTerminalShellOptions` |
| `IntegratedTerminalShellOption` | `IntegratedTerminalShellOption` |

可执行检查由调用方注入闭包；本 crate 不访问文件系统，也不读 `process.platform`。

## 2. 行为

- 非 `win32` 平台恒返回空数组。
- Windows 第一项固定为 CMD：`ComSpec`（环境键大小写不敏感）trim 后非空则使用，
  否则 `cmd.exe`；`source="system"`。
- Git Bash 查找顺序：
  1. `C:\Program Files\Git\bin\bash.exe`、`C:\Program Files (x86)\Git\bin\bash.exe`；
     命中时 `source="system"`。
  2. 在 `PATH`（`;` 分隔，忽略空白项）中按 `PATHEXT` 顺序拼接 `git<ext>`；`PATHEXT`
     缺省 `.COM/.EXE/.BAT/.CMD`，且缺少 `.exe` 时前置 `.exe`。找到 git 可执行后，
     从其目录推导 `..\bin\bash.exe` 与 `..\..\bin\bash.exe`，命中时 `source="path"`。
- 输出字段固定为 `dialect/id/label/path/source`；id 分别为 `cmd:<path>` 与
  `git-bash:<path>`。
- Windows 路径拼接/`dirname` 使用 win32 语义，处理 `.`/`..` 归一化。

## 3. 已知差异

1. Node `path.win32` 对 UNC、设备路径、混合分隔符的边角行为未完整复刻；夹具覆盖
   常规盘符路径、空格路径与相对 `..` 推导。
2. 默认实现的可执行检查是 `accessSync(X_OK)`；Rust 侧必须由调用方注入等价检查。

## 4. 验收

- `pnpm integrated-terminal-shells:fixtures` →
  `crates/integrated-terminal-shells/fixtures/shell-options.json`
  - 11 组：非 Windows、默认 CMD、ComSpec trim/大小写、固定 Git Bash、
    PATH 第一/第二推导、只有 git 无 bash、默认 PATHEXT 顺序、空 PATH 段与空格路径、
    固定路径优先于 PATH。
- `cargo test -p integrated-terminal-shells`（2 条 Rust 单测 + 1 条 TS 回放）。
- `cargo clippy -p integrated-terminal-shells --all-targets -- -D warnings`。
