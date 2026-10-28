# 终端 PTY 下沉到壳（2026-10-26 立项）

前置：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md`（Rust 化收尾）、
`docs/specs/tauri-renderer-host.md` §22.4/§22.5（Host 崩溃自愈）。

## 1. 动机

终端的 PTY（node-pty）由 Host 进程托管，带来一个与自愈架构直接冲突的韧性缺陷：
**Host 崩溃/重生时终端会话全部死亡**（重生路径的 `taskkill /T` 连 pty 子进程一起杀）。
把 PTY 进程下沉到 Tauri 壳（Rust `portable-pty`）后：

- 终端在 Host 崩溃/重生周期里存活（壳不受 Host 生命周期影响）；
- Host 少一个最重的原生依赖（node-pty 及其平台分发包）。

## 2. 所有者与职责切分（关键决策）

| 职责 | 所有者 | 理由 |
| --- | --- | --- |
| **PTY 进程**（spawn/write/resize/kill、输出泵） | **壳**（`crates/platform/src/commands/terminal.rs`，portable-pty） | 进程所有权 = 韧性目标本身 |
| **Profile**（shell 选择、字体/主题、cwd 回退、windowsPty 信息） | **Host**（`ITerminalService.profile()`，由 `create` 抽出） | 设置、字体文件探测、shell 解析逻辑已在 Host（415 行 terminalProfile），Rust 复制必然漂移 |
| **组装与消费** | **渲染层**（`desktopTerminalBridge.ts` 的 shell-backed `ITerminalService`） | create = Host profile + 壳 spawn；事件走 Tauri event 按 id 过滤 |

事件序：

```
渲染层 shellTerminalService.create({cols,rows,cwd,sessionKey?})
  ① host.terminalService.profile({cwd})      → {shell, fontFamily, fontSize?, theme?,
  │                                             fontFamilySource, windowsPty?, cwd}
  ② invoke("terminalCreate", {key: sessionKey?})
     → {id}（壳 portable-pty spawn；输出泵线程启动）
     → 或 key 命中活会话：{id, replay, reattached:true}（不 spawn）
  ③ 返回 {id, ...profile, replay?, reattached?}
渲染层 write/resize/dispose → invoke terminalWrite/Resize/Dispose
壳 输出线程 → emit("terminal://data", {id, data})；退出 → emit("terminal://exit", {id, code})
渲染层 onDynamicData/onDynamicExit(id) → listen("terminal://*") 按 id 过滤
```

## 3. 规则

1. **Host 的 PTY 路径保留**：Web/远程线（`packages/server`、mode-server-cli）继续用
   node-pty——下沉只发生在 Tauri 渲染层的装配点（accessor 覆盖），接口与其余实现不动。
2. **`profile()` 是 `ITerminalService` 的可选成员**：Host 实现之；无该成员的旧实现
   （远端 workspace）由渲染层回退到 Host 原路径（ capability 探测，不伪装成功）。
3. **壳侧会话即壳的资产**：Host 死亡/重生**不**触碰终端；终端只随壳退出（或用户 dispose）。
   监督路径的 `taskkill /T` 因此不再影响终端子进程（它们不再是 Host 的子进程）。
4. **数据面**：输出经 Tauri event（JSON 字符串）而非 MessagePort 原始字节——交互式终端
   帧小、频率可控，换通道省一整套注入面；高吞吐场景（cat 大文件）如实测卡顿，再立项
   MessagePort 通道（登记为开放项，不预先建设）。
5. **env 口径**：壳 spawn 继承壳进程环境（与 Host 同源，均来自 mode-tauri）；Host 的
   darwin PATH 修补不适用（本项目终端仅在 Windows 提供先例，mac 走 Host 原路径）。
6. **新依赖**：`portable-pty`（wezterm，MIT）——licenses 台账按既有循环在 CI 重生成后
   回填（同 `a92d42f4`）。
7. **跨整页 reload 的 reattach（2026-10-27，P2-4）**：`ITerminalService.create` 增加
   可选 `sessionKey`；壳侧 `TerminalSessionManager` 维护 key→id 索引，命中活会话时
   返回原 id + 有界输出回放（256 KB，UTF-8 边界），不重新 spawn。key 在 dispose /
   进程退出时按 id 清理；`terminalDispose` 先摘 key，避免命中正在被杀死的会话。
   渲染层先写 `replay` 再订阅 live data；回放与 live 之间的极小缺口不伪造完整
   scrollback。当前 key 口径：
   - side pane：`side:<workspaceKey>:<tab sessionId>`（tab 由 workspace 布局恢复）；
   - 底部终端：仅初始 tab `bottom:<workspaceKey>:1` 带 key；额外 tab 的 UI 状态不持久化，
     不带 key，reload 后仍按旧行为消失，避免留下无法回收的孤儿 PTY。

## 4. 已知边界

- 终端内运行的进程是壳的子进程：壳退出（用户退出/更新重启）仍然会带走它们——与旧线
  一致，本设计只承诺「Host 崩溃不带走」。
- `resolveTerminalShell/cwd` 的语义在 Rust 侧重写（pwsh→powershell→ComSpec→cmd 探测、
  cwd 失效回退 HOME），与 Host 共享**行为口径**而非代码；对拍用例锚住（shell 选择顺序、
  cwd 回退顺序）。
- Host 原路径（node-pty）在 Tauri 渲染层不再被调用，但服务与其实现保留（Web/远程线）。
- 整页 reload 后只有带 `sessionKey` 的会话重新挂接；无 key 的会话（额外底部 tab）随
  reload 消失。壳侧不主动列/杀所有无 key 会话，避免误伤其他窗口；若后续要恢复全部
  底部 tab，需要先持久化 tab 列表再补 `terminalPrune`。
