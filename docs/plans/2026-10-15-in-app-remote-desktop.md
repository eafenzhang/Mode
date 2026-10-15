# 计划：应用内远程桌面（恢复自研媒体面 + 补齐缺口）

配套 spec：[docs/specs/in-app-remote-desktop.md](../specs/in-app-remote-desktop.md)。
本文只讲**怎么落地**：决策默认项、任务分解、验证口径、风险。

## 先看这段：我按推荐默认做的 4 个决定（评审时可推翻任一条）

1. **恢复而非重写**。二代的媒体链路（`crates/mode-cua-ax/src/rd_media/` 3,882 行、helper 媒体泵、
   main/host 协调器、UI 会话面 2,762 行、shared 信令与 UI 输入采集）本就是**我方自有代码**，
   且在真机上逐项修过根因（无光标 → DUPL 零投递 + user32 兜底、异步 MFT、换屏 IDR、动态降档）。
   重写没有收益：我们对它的架构没有具体的、需要推翻的不满，全部不满（剪贴板/音频/多屏/SAS/跨网）
   都是**能力缺口**，可以在恢复后增量补。
   → 兜底：若你要求重写，spec 的行为面不变，只换 P0 的起点（成本按 3–5 倍计）。
2. **移除系统 RDP 路径（2026-10-15 追加决定：用户明确「自研的无需 RDP 了」）**。远程桌面只保留应用内
   一种形态：v0.0.42 的启动器、连接选项、`systemRdp` 能力位与注册表探测、`rd/connect|get-sessions|
   disconnect|*-rdp-options` 通道、会话列表 UI 及其测试**整体删除**。
   → 好处：家庭版可被控（RDP 做不到）、应用内画面、应用级治理全部回来；代价：剪贴板/音频/多屏同时/SAS
   回到自研待补状态，且要接受自研媒体链的长期维护。
   → 关键简化：因为 54aeb907 之后主线只多了一个纯文档提交，**`git revert 54aeb907` 可一次性完成
   「恢复自研 + 删除 RDP」**，没有两套实现同居一文件的对账问题（这也是本决定最大的工程收益）。
3. **SAS（Ctrl+Alt+Del）本轮明示不支持**。Windows 禁止用户态合成 SAS，需被控端服务 + `SendSAS` 策略；
   我们**没有任何 Windows 服务设施**（已核实：仓库里搜不到服务安装/`SendSAS` 代码，helper 是用户态进程），
   而 RustDesk 能做这类事正是因为它装了一个 Windows 服务组件。服务化 = 安装器 + UAC + 权限模型 +
   安全评审，属独立立项，不在本轮。
4. **缺口优先级 = 剪贴板（文本）→ 音频（被控→主控）→ 多屏铺开**，逐项单独交付。
   文件剪贴板、麦克风上行、文件传输、虚拟显示器、多会话 tab 全部后置（P4）。

## P0：恢复基线（目标 = 与 v0.0.35 时代能力对齐）

工作量不在「写」，在**对账**：删除集可以机械恢复，但 62 个文件里现在同时住着自研实现与 RDP 实现，
必须人工合并。

### 恢复集（`git checkout 54aeb907^ -- <paths>`，68 个文件，机械恢复）

| 层 | 内容 |
| --- | --- |
| Rust | `rd_media/`（capture 1938 / encode 1262 / napi 549 / state 121 / mod）、`tests/rd_media.rs`、`lib.rs` 挂载、`Cargo.toml` 的 DXGI/MF/napi4 features |
| helper | `helper/mediaPump.mjs`、`helper/server.mjs` 媒体段、`broker.js` rd_media 白名单、`test/media-pump.test.mjs` |
| services | `cua-permission-broker/cuaRdMedia.ts` 与它的测试、`node.ts` 的媒体端口接线 |
| main | `remoteDesktop/{coordinator,controllerSession,consentWindow,indicatorWindow,windowsContent,monitors,mediaCoordination,peerSessionOpenOutcome,sessionState}.ts`、`settings.ts`（含逐设备授权） |
| host | `remoteDesktopServer.ts`、`remoteDesktopMediaRelay.ts`、`remoteDesktopAdmission.ts`、`remoteDesktopPeerClient.ts`、`inputConversion.ts` |
| server | `remoteDesktopTicket.ts`、`http.ts` 的三条路由（session POST / 信令 WS / 媒体 WS）与测试 |
| shared | `remote-desktop.ts`（信令 union、输入 schema、坐标与键盘纯函数、画质档位、codec）与测试 |
| ui | `RemoteDesktopSession.tsx`、`nativeMediaSession.ts`、`useRemoteDesktopSession.ts`、`sessionReducer.ts`、`videoFit.ts`、`codecString.ts`、`codecFallback.ts`、`adaptiveBitrate.ts`、`sessionStats.ts`、`InputCapture.ts`、`firstFrameWatchdog.ts`、`reconnectPolicy.ts`、`sessionReconnectController.ts` 与它们 11 个测试 |
| docs | 恢复 `lan-remote-desktop.md` 的**规则段落**进新 spec（已完成，见 spec 的产品规则/事件顺序），旧两份 spec 不单独回灌 |

### 机制：一次 revert 同时完成「恢复」与「移除」

54aeb907 是本功能在主线上的唯一分水岭，其父提交 `c855b32f` 就是二代的完整状态；而 54aeb907 之后
主线只多了一个**纯文档**提交（`f94480c0`）。因此 **`git revert 54aeb907`** 一次做完两件事：

- 68 个被删文件全部回来（媒体链路、会话面 UI、同意/授权/指示、helper 媒体泵、Rust `rd_media`、
  server 票据与路由、shared 信令与画质档位、11 个 UI 测试、`tests/rd_media.rs`）；
- 14 个 RDP 专属新文件全部消失（`rdpLauncher`/`rdpOptions`/`systemRdpState`/`remoteDesktopSharing`/
  `remoteDesktopPeerProbe`、RDP 连接选项与会话列表组件、RDP 测试与夹具）；
- 62 个共享文件回到二代形态（`host/index.ts`、`main/index.ts`、`rd/*` IPC 面、两份镜像消息、
  i18n 三份、能力位、`language-policy-allowlist`、`third-party/inventory.json`）——**不存在两套实现同居一文件的对账问题**。

需人工收口的三处冲突：

1. `docs/specs/lan-remote-desktop.md`：revert 会带回 185 行旧产品 spec。**保留历史存根**
   （指向 `in-app-remote-desktop.md`，记录三代变更轨迹），旧正文以 git 为准。
2. `docs/specs/remote-desktop-system-rdp.md`：revert 会删除它。**接受删除**——其内容与 v0.0.42 行为
   记录在 `54aeb907` 与本次提交信息里，不留长文以免出现第二份「看起来有效」的权威。
3. `docs/specs/lan-remote-desktop-native-media.md` 与两份 `2026-10-12-*` 计划会一并回来：
   前者内容已并入新 spec，**改为存根或删除**（避免三份权威）；后两者是历史计划，保留即可。

### 任务

- **T0.1** `git revert 54aeb907`，收口上述文档冲突；`git status` 复核：只动该提交涉及的文件，
  工作区里三处与本功能无关的改动（对话镜像挂载点、分组绿点、两份 spec）不得被波及。
- **T0.2** 门禁与测试：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
  shared / services / server / desktop / ui / mode-cua 测试全绿。
- **T0.3** `node scripts/check-language-policy.mjs`（媒体泵与其测试的清单项随 revert 回来，
  陈旧项应为 0）与第三方台账 inputs 新鲜度校验。
- **T0.4** Rust：`cargo test -p mode-cua-ax`。**本机无法链接**（Git Bash 的 `link.exe` 遮蔽 MSVC 且无
  VS/Windows SDK），仍由 CI 判定，报告中如实说明。
- **T0.5** 提交信息写清这是 v0.0.42 的**反向变更**（移除 RDP 路径、恢复自研），并列出对升级用户可见的
  行为变化（供发版说明）。

**P0 验收**：spec 实机清单里「家庭版可连」「应用内画布 + 操作栏」「同意/授权/指示」「延迟跟手」
「多显示器切换」「画质档位」通过；`systemRdp`、RDP IPC 通道、RDP UI 与 RDP 测试在仓库内**零残留**。

## P1：补齐四个缺口（逐项独立交付）

- **T1.1 剪贴板文本**（拆两段，各自可独立验收）：
  - **T1.1a 被控 → 主控（已完成，2026-10-15 下午）**：Rust 增 `clipboard_read_text`/`clipboard_write_text`
    两个 napi 导出（复用既有 `clipboard.rs`）；helper 媒体泵新增 kind5（变化才上报，会话建立时以本端
    既有内容为基线——**同步从「变化」开始**，连上不顶掉对端剪贴板）与 kind6（落地主控文本）；
    宿主媒体中继持被控侧的门（shared 回环抑制状态机）并把采纳的文本下行 `{t:"clip"}`；
    renderer 侧 `clipboardSync.ts` 持有主控侧的门并写本机剪贴板；会话面操作栏开关（默认开）经
    媒体 WS 上行 `clip-mode`。文本上限 128K 码元；空文本不参与。
  - **T1.1b 主控 → 被控（未开始）**：需要主控侧「观测本机剪贴板变化」的读取面（Electron `clipboard.readText`
    经 IPC，四处同步：main 常量 → preload → shared platform → renderer 桥），renderer 轮询后
    经同一 `{t:"clip"}` 上行；被控侧复用既有 kind6。语义门与开关已在 T1.1a 就位，故这一段是纯接线。
- **T1.2 音频（被控 → 主控）**：helper 内 WASAPI 采集 + Opus 编码 → 与视频同一传输（新 kind）→
  renderer 解码播放（`AudioDecoder`/WebAudio）；开关默认开。新增依赖需过第三方台账（Opus 是 BSD 系，风险低）。
  验收：被控端播放有声音、关闭即静音、不劣化视频时延。
- **T1.3 多显示器铺开**：在既有「选一块」之上加「全部铺开」（`monitors.ts` 扩展 + 会话面入口）；
  实现层二选一（多路流 / 单张虚拟屏合成）在实现时定，spec 只约定行为。验收：两块屏同时可见可操作。
- **T1.4 SAS 现状明示**：会话面操作栏显示「本路径不支持 Ctrl+Alt+Del」（系统 RDP 路径已移除，
  没有可引导的替代入口）；不静默失败，也不做成假按钮。验收：UI 明示且不影响其它操作。

## P2：质量（传输层抽象出替换点，为 UDP/抗丢包预留）

- 传输接口化（帧源/加密/通道三处可替换），默认实现仍是长度前缀二进制 + AES-GCM + 本机 TCP + LAN WS。
- 动态分辨率与弱网阈值在 LAN/公网两类实机上重新标定（二代阈值是 LAN 口径）。
- 编码能力观测与缓存沿用二代（`rd-encoder-capability.json`）。

## P3 / P4（另立 spec 或独立评估）

- P3 公网：rendezvous + 中继 + 打洞（对标 RustDesk 的服务端形态，自研，不引其代码）。
- P4 周边：文件传输、虚拟显示器、多会话 tab、打印机重定向。
- 明确不做：macOS/Linux 对端、Web 客户端、WOL、跨平台虚拟屏、自研 RDP 协议。

## 风险与注意事项

1. **revert 的收口面**：`lan-remote-desktop.md`（保留存根）与 `remote-desktop-system-rdp.md`（接受删除）
   两处文档冲突必须人工处理；revert 还会带回 `lan-remote-desktop-native-media.md` 与两份 `2026-10-12-*` 历史计划，
   前者要收敛（并入新 spec 后存根或删除），否则仓库里会出现三份「远程桌面权威」。
2. **v0.0.42 混跑兼容**（本轮最容易出事的地方）：`systemRdp` 能力位与 RDP 通道被删后，
   - **旧主控（v0.0.42）连新被控**：旧主控只走 RDP 路径、要读 `systemRdp`，新被控不再报告 → 设备卡会置灰/报「未开启」，表现为「连不上」；
   - **新主控连旧被控（v0.0.42）**：`remoteDesktop` 位语义回到「应用内可用」，但对方的应用内代码已删 → 发起后必然失败。
   缓解：能力位语义与 `protocolVersion: 2` 门必须按 spec 落地，并在测试里钉住；发版说明里明确「新旧版本互控不支持，请两端同时升级」。
3. **恢复的代码相对当前主线未经编译**：54aeb907 之后主线只有纯文档提交（`f94480c0`），所以 TS 面预期可直接绿；
   Rust 面仍要看 CI（本机无法链接）。`desktop main` 不在 `pnpm typecheck` 图内，必须单独扫或完整打包一次。
4. **SAS 与 UAC 提权窗口**：`input.rs` 已有 `uipi_preflight`，被控端 UAC 提示符上的输入**注定不可达**
   （与 SAS 同因，需要被控端服务才可能突破）。UI 应把它当已知限制呈现，不要试图绕过。
5. **不要读 RustDesk 源码**（AGPL + 仓库既有纪律）。对标只用公开资料与黑盒行为。

## 待你拍板的两件事

1. **自研总开关在老用户升级后的默认值**：默认仍为关（与现状一致，需用户显式打开）。
2. **发版说明口径**：本次推送会触发自动发版（v0.0.43）。因为它是 v0.0.42 的反向变更（移除系统 RDP、
   恢复自研），且剪贴板/音频/SAS 要等 P1 才有，建议在 Release 说明里写清这三点；若你想先 [skip release]
   等 P1 一起发，告诉我即可。

## 实现进度

- [x] **P0 完成（2026-10-15）**：`revert 54aeb907`（`86830a9f`）恢复自研媒体面并移除系统 RDP 路径；
  验证全绿（typecheck/lint/architecture、shared 71 / services 328 / server 30 / desktop 72 / ui 149 / mode-cua 134）；
  顺带修正 ui `remoteDesktopDecoder` 的过期断言并把 ui 测试接进 CI verify。
- [x] **T1.1a 完成（2026-10-15）**：剪贴板文本「被控 → 主控」端到端（Rust 两个 napi 导出、helper kind5/kind6、
  宿主中继语义门、renderer 落地、操作栏开关 + i18n 三份）；shared 回环抑制状态机与 5 条单测、5 条 UI 单测。
  **Rust 侧本机无法编译，由 CI 判定**。
- [ ] T1.1b 主控 → 被控（纯接线，见上）。
- **已知缺口（T1.1a 遗留）**：helper 侧（`mediaPump` 的剪贴板轮询、kind5 上报、kind6 落地）没有单测——
  需要给现有 `packages/mode-cua/test/media-pump.test.mjs` 加假 addon/假管道的用例；同时注意
  `packages/mode-cua` 的测试**不在 CI**（verify 只跑 shared/services/server/ui/core/desktop），
  所以即使补了也不会被自动守住。两件事一起做才算把这段收干净。
- [ ] T1.2 音频、T1.3 多屏铺开、T1.4 SAS 明示。
- [ ] 实机验收（两台 Windows，含家庭版）——**P0 与 T1.1a 都还没有实机证据**。
