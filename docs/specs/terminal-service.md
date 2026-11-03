# Spec：终端服务与 `terminal` 频道（Rust）

> 状态：W3-5 剩余项（PTY 生命周期与 node-pty 编排）的迁移目标。
> TS oracle：`packages/services/src/terminal/terminalService.ts`（510 行），
> 接口 `packages/services/src/terminal/terminal.ts`，纯解析面见
> `terminal-service-resolution.md`、`terminal-profile.md`、`integrated-terminal-shells.md`。
> 壳侧 PTY 下沉的背景与所有者决策见 `terminal-pty-in-shell.md`。

## 1. 范围

| Rust 入口 | TS 对应 | 说明 |
| --- | --- | --- |
| `terminal-session`（crate） | `node-pty` + `TerminalInstance` | PTY 进程、输出泵、退出监视、稳定 key 重挂接、有界回放 |
| `terminal-service`（crate） | `createTerminalService` | profile 组合（shell/cwd/env/font/theme/windowsPty）与会话编排 |
| `host-channels::terminal` | `ServiceChannels.Terminal` 频道 | 方法 `profile/create/write/resize/dispose/disposeByKey` 与事件 `onDynamicData/onDynamicExit` |

桌面 Tauri 渲染层仍走 `terminalCreate/terminalWrite/terminalResize/terminalDispose/
terminalDisposeByKey` 命令（`platform::terminal` 的 PTY 管理器）；本文件的频道是**壳内
Host 服务面**的入口，供 `MODE_HOST_INPROCESS_CHANNELS=terminal` 以及后续 Web/远程线使用。
Node Host 的 `terminalService` 在删除前始终是行为 oracle。

## 2. 所有者与边界

```text
renderer / web / remote
        │ RPC: terminal.create|write|resize|dispose|disposeByKey
        ▼
host-channels::TerminalChannel ──► terminal-service
                                      ├─ profile：terminal-service-resolution + terminal-profile + setting
                                      └─ 会话：terminal-session（portable-pty，唯一所有者）
                                                │ data / exit
                                                ▼
                                       per-id 订阅者 → RPC 事件
```

- **唯一所有者**：`terminal-session::TerminalSessionManager` 持有 PTY 进程表与 key 索引；
  频道、桌面命令面都只能通过它的公开方法操作，不另建会话表。
- **profile 不复制**：shell 选择、cwd 回退、env 补丁、字体/主题组合、Windows PTY 信息
  全部复用已对拍的纯逻辑 crate；`terminal-service` 只做文件探测与编排。
- **事件顺序**：同一会话内 data 事件按输出泵顺序；进程退出后先发 exit，再从会话表移除。
  `create` 返回后新会话才可能收到事件（订阅按 id 建立）。
- **不变量**：
  - 一个 `sessionKey` 同时最多映射一个活会话；命中活会话时 `create` 返回原 id 与回放，
    不新建进程。
  - `dispose` 幂等；已退出/已释放的 id 再次 `dispose` 不报错。
  - `write`/`resize` 对未知 id 返回 `Terminal not found: {id}`。
  - 会话退出后 key 索引按 id 清理，同 key 的下一次 `create` 必新建。

## 3. 方法与事件

### 3.1 `profile({ cwd? }) → TerminalProfileInfo`

```json
{
  "shell": "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
  "cwd": "D:\\workspace",
  "fontFamily": "Cascadia Mono, ...",
  "fontSize": 14,
  "theme": { "background": "#..." },
  "fontFamilySource": "system",
  "windowsPty": { "backend": "conpty", "buildNumber": 19045 }
}
```

- `shell`：Win32 依次 `pwsh.exe`、`powershell.exe`、`ComSpec`、`cmd.exe`；POSIX 依次
  `SHELL`、`/bin/zsh`、`/bin/bash`、`/bin/sh`；每个候选都必须可执行。
- `cwd`：传入值 → `HOME` → 系统 home → `/`，取第一个可用目录。
- `fontFamily`/`fontSize`/`theme`：`setting.json` 的 `terminalFontFamily` 优先（source=custom）；
  否则在 `terminalInheritSystemProfile !== false` 时按 TS 探测器顺序取第一个命中
  （Windows Terminal → VS Code → iTerm2 → macOS Terminal → kitty → alacritty）；
  都没有则 fallback 字体栈。
- `windowsPty`：仅 win32 返回 `{ backend: "conpty", buildNumber? }`；build number 来自
  系统 release（注册表 `CurrentBuildNumber`），读不到时省略。
- 错误：无可用 shell / 无可用 cwd 时返回与 TS 相同的错误串。

### 3.2 `create({ cols, rows, cwd?, sessionKey? }) → { id, ...profile, replay?, reattached? }`

1. 解析 profile（与 `profile` 同一实现）。
2. 用 `resolveTerminalEnv` 生成 env（TERM/COLORTERM/locale/PATH），传给 PTY。
3. `sessionKey` 命中活会话时返回原 id、`replay`（≤256 KiB，按 UTF-8 边界截断）与
   `reattached=true`；否则新建并登记 key。
4. spawn 失败返回
   `Failed to start terminal with shell '{shell}' in '{cwd}': {message}`。

### 3.3 `write({ id, data })` / `resize({ id, cols, rows })` / `dispose({ id })`

- `write` 写入 PTY 并 flush；未知 id 返回 `Terminal not found: {id}`。
- `resize` 调 PTY resize；未知 id 同上。
- `dispose` kill 子进程并按幂等成功处理；exit 事件由退出监视线程统一发出。

### 3.4 `disposeByKey({ sessionKey }) → void`

按稳定 key 释放会话；不存在时幂等成功。用于关闭带 key 的 tab，避免只收起 UI 而泄漏 PTY。
（Tauri 命令面另返回被释放的 id 供渲染层清理 emitter；频道面与 `ITerminalService` 一致返回 void。）

### 3.5 事件 `onDynamicData(id)` / `onDynamicExit(id)`

- `onDynamicData`：UTF-8 解码后的输出块（跨块的多字节序列只解一次；非法字节丢弃）。
- `onDynamicExit`：退出码；发出后会话与 key 从表中移除。
- 订阅无效 id 不抛错，返回永不触发的事件流（与 `file-watcher` 一致）。

## 4. 环境与平台差异

- `resolveTerminalEnvWith` 的补丁：`TERM=xterm-256color`、`COLORTERM` 缺省 `truecolor`、
  GUI 启动缺失/`C`/`POSIX` locale 补 UTF-8、macOS 补 Homebrew/系统 PATH、
  `CI=1 + TERM=dumb` 时移除 `CI`。
- Windows：`portable-pty` 使用系统 ConPTY；TS 的 `useConptyDll` 实验路径与
  `shouldFallbackFromConptyDll` 在 Rust 无对应开关，命中 DLL 加载错误的回退逻辑不适用。
- macOS：TS 的 node-pty `spawn-helper` 权限修复不适用（portable-pty 自带启动路径）；
  plist 探测仍用 `plutil -convert json` 读取，失败按 best-effort 跳过。
- 内存诊断计数器（TS `registerMemoryDiagnosticsProvider("terminal")`）未迁移；会话泄漏可经
  频道方法观测，后续按需接入统一诊断面。

## 5. 失败语义

| 场景 | 行为 |
| --- | --- |
| 无可用 shell | `No usable Windows shell found for terminal startup` / `No usable shell found for terminal startup` |
| 无可用 cwd | `No usable working directory found for terminal startup` |
| spawn 失败 | `Failed to start terminal with shell '{shell}' in '{cwd}': {message}` |
| write/resize 未知 id | `Terminal not found: {id}` |
| dispose 未知 id | 幂等成功 |
| disposeByKey 未知 key | `{ id: null }` |
| 订阅未知 id | 永不触发的事件流，不抛错 |
| 输出泵 EOF/读错误 | 停止输出；进程退出由退出监视线程发 exit |

## 6. 迁移边界与验收

- Node Host 的 `terminalService` 保留为 oracle，直到本频道在真机开关下通过正常/错误/取消/
  事件四类用例；关闭开关仍走 Node 路径。
- 频道名必须与 TS `ServiceChannels.Terminal` 一致（`host-channels/tests/channel_names.rs`）。
- Rust 测试：
  - `cargo test -p terminal-session`：UTF-8 跨块、回放上限、key 重挂接、spawn/write/exit。
  - `cargo test -p terminal-service`：shell/cwd 回退、env 补丁、字体 profile 组合、
    Windows PTY 信息、错误串。
  - `cargo test -p host-channels`：频道名漂移守卫与事件订阅。
- 真机验收（待做）：`MODE_HOST_INPROCESS_CHANNELS=terminal` 下由渲染层/远程端创建、
  写入、resize、关闭；断链后同 key 重挂接拿到回放；关闭开关回归 Node 路径。
